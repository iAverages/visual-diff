use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub status: FileStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

impl FileStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "?",
        }
    }
}

pub fn repository_root(path: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .map_err(|error| format!("Could not run git: {error}"))?;

    if !output.status.success() {
        return Err(format!("{} is not a Git repository", path.display()));
    }

    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

pub fn changed_files(repo: &Path) -> Result<Vec<ChangedFile>, String> {
    let output = Command::new("git")
        .args([
            "-c",
            "status.renames=false",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("Could not read Git status: {error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }

    let mut files = parse_status(&output.stdout);
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

pub fn file_versions(repo: &Path, file: &ChangedFile) -> Result<(Vec<u8>, Vec<u8>), String> {
    let before = if matches!(file.status, FileStatus::Added | FileStatus::Untracked) {
        Vec::new()
    } else {
        let spec = format!("HEAD:{}", file.path.to_string_lossy());
        let output = Command::new("git")
            .args(["show", &spec])
            .current_dir(repo)
            .output()
            .map_err(|error| format!("Could not read HEAD version: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "Could not read HEAD version of {}: {}",
                file.path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        output.stdout
    };

    let working_path = repo.join(&file.path);
    let after = if working_path.is_file() {
        fs::read(working_path)
            .map_err(|error| format!("Could not read {}: {error}", file.path.display()))?
    } else {
        Vec::new()
    };

    Ok((before, after))
}

pub fn visit_file_versions(
    repo: &Path,
    files: &[ChangedFile],
    mut visit: impl FnMut(&ChangedFile, &[u8], &[u8]),
) -> Result<(), String> {
    let mut child = Command::new("git")
        .args(["cat-file", "--batch"])
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start git cat-file: {error}"))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));

    for file in files {
        let before = if matches!(file.status, FileStatus::Added | FileStatus::Untracked) {
            Vec::new()
        } else {
            writeln!(stdin, "HEAD:{}", file.path.to_string_lossy())
                .and_then(|_| stdin.flush())
                .map_err(|error| format!("Could not request Git object: {error}"))?;

            let mut header = String::new();
            stdout
                .read_line(&mut header)
                .map_err(|error| format!("Could not read Git object header: {error}"))?;
            let size = header
                .split_whitespace()
                .nth(2)
                .and_then(|size| size.parse::<usize>().ok())
                .ok_or_else(|| format!("Could not read HEAD version of {}", file.path.display()))?;
            let mut bytes = vec![0; size];
            stdout
                .read_exact(&mut bytes)
                .map_err(|error| format!("Could not read Git object: {error}"))?;
            let mut newline = [0];
            stdout
                .read_exact(&mut newline)
                .map_err(|error| format!("Could not finish reading Git object: {error}"))?;
            bytes
        };

        let working_path = repo.join(&file.path);
        let after = if working_path.is_file() {
            fs::read(&working_path)
                .map_err(|error| format!("Could not read {}: {error}", file.path.display()))?
        } else {
            Vec::new()
        };
        visit(file, &before, &after);
    }

    drop(stdin);
    let status = child
        .wait()
        .map_err(|error| format!("Could not finish git cat-file: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("git cat-file failed".to_owned())
    }
}

fn parse_status(bytes: &[u8]) -> Vec<ChangedFile> {
    let entries: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .collect();
    let mut files = Vec::new();
    let mut index = 0;

    while index < entries.len() {
        let entry = entries[index];
        if entry.len() < 4 {
            index += 1;
            continue;
        }

        let status_bytes = &entry[..2];
        let status = match status_bytes {
            b"??" => FileStatus::Untracked,
            status if status.contains(&b'D') => FileStatus::Deleted,
            status if status.contains(&b'R') => FileStatus::Renamed,
            status if status.contains(&b'A') => FileStatus::Added,
            _ => FileStatus::Modified,
        };
        let path = PathBuf::from(String::from_utf8_lossy(&entry[3..]).into_owned());
        files.push(ChangedFile { path, status });

        index += if status_bytes.contains(&b'R') || status_bytes.contains(&b'C') {
            2
        } else {
            1
        };
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_common_porcelain_entries() {
        let files = parse_status(b" M src/main.rs\0?? new file.json\0D  old.png\0");

        assert_eq!(
            files,
            vec![
                ChangedFile {
                    path: "src/main.rs".into(),
                    status: FileStatus::Modified,
                },
                ChangedFile {
                    path: "new file.json".into(),
                    status: FileStatus::Untracked,
                },
                ChangedFile {
                    path: "old.png".into(),
                    status: FileStatus::Deleted,
                },
            ]
        );
    }

    #[test]
    fn consumes_rename_source_path() {
        let files = parse_status(b"R  renamed.txt\0original.txt\0 M next.txt\0");

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, PathBuf::from("renamed.txt"));
        assert_eq!(files[1].path, PathBuf::from("next.txt"));
    }

    #[test]
    fn visits_head_and_working_versions_in_one_batch() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let repo = std::env::temp_dir().join(format!("visual-diff-{unique}"));
        fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&repo)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        fs::write(repo.join("asset.bin"), b"before").unwrap();
        git(&["add", "asset.bin"]);
        git(&["commit", "--quiet", "-m", "initial"]);
        fs::write(repo.join("asset.bin"), b"after").unwrap();
        let file = ChangedFile {
            path: "asset.bin".into(),
            status: FileStatus::Modified,
        };
        let mut versions = None;

        visit_file_versions(&repo, &[file], |_, before, after| {
            versions = Some((before.to_vec(), after.to_vec()));
        })
        .unwrap();

        assert_eq!(versions, Some((b"before".to_vec(), b"after".to_vec())));
        fs::remove_dir_all(repo).unwrap();
    }
}
