# Visual Diff

A visual comparison tool designed for reviewing assets and large JSON diffs in a local Git repository.

## Run

```sh
nix run
```

For development:

```sh
nix develop
cargo run
```

## Features

- Recent repository list
- Staged, unstaged, deleted, and untracked changes
- Unified and side-by-side text diffs
- JSON formatting, syntax highlighting, and ignored keys
- Side-by-side and slider image comparisons
- Filtering for visually unchanged images
- Dark mode
