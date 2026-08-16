mod git;

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use ::image::GenericImageView;
use git::{ChangedFile, FileStatus};
use iced::{
    Alignment, Border, Color, Element, Fill, Font, Length, Padding, Point, Rectangle, Renderer,
    Size, Task, Theme, font, mouse,
    widget::{
        button, canvas, checkbox, column, container, horizontal_rule, image, rich_text, row,
        scrollable, slider, span, text, text_input,
    },
};
use serde_json::Value;
use similar::{ChangeTag, TextDiff};

const SIDEBAR_WIDTH: f32 = 300.0;
const CODE_SIZE: f32 = 13.5;
const CONTEXT_LINES: usize = 3;
const MONO: Font = Font {
    family: font::Family::Name("JetBrains Mono"),
    weight: font::Weight::Medium,
    stretch: font::Stretch::Normal,
    style: font::Style::Normal,
};

#[derive(Debug, Clone)]
struct PreparedDiff {
    rows: Vec<DiffRow>,
}

#[derive(Debug, Clone)]
struct DiffRow {
    left: DiffCell,
    right: DiffCell,
    changed: bool,
}

#[derive(Debug, Clone)]
struct DiffCell {
    marker: &'static str,
    line: String,
    kind: DiffKind,
    tokens: Vec<(String, JsonToken)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffKind {
    Added,
    Removed,
    Unchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibleRow {
    Line(usize),
    Collapsed { start: usize, count: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageClassification {
    PixelIdentical,
    VisuallyChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextLayout {
    SideBySide,
    Unified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageLayout {
    Reveal,
    SideBySide,
}

#[derive(Debug, Clone)]
struct ImagePreview {
    before: image::Handle,
    after: image::Handle,
    before_size: Size<u32>,
    after_size: Size<u32>,
}

#[derive(Debug, Clone)]
struct ImageReveal {
    preview: ImagePreview,
    split: f32,
}

#[derive(Debug, Default)]
struct RevealState {
    dragging: bool,
}

impl canvas::Program<Message> for ImageReveal {
    type State = RevealState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor.position_over(bounds).is_some() =>
            {
                state.dragging = true;
                (
                    canvas::event::Status::Captured,
                    split_message(cursor, bounds),
                )
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => (
                canvas::event::Status::Captured,
                split_message(cursor, bounds),
            ),
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.dragging =>
            {
                state.dragging = false;
                (canvas::event::Status::Captured, None)
            }
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let local_bounds = Rectangle::new(Point::ORIGIN, bounds.size());
        let before_bounds = contain_bounds(self.preview.before_size, local_bounds);
        let after_bounds = contain_bounds(self.preview.after_size, local_bounds);
        let split_x = bounds.width * self.split;

        frame.draw_image(after_bounds, canvas::Image::new(self.preview.after.clone()));
        frame.with_clip(
            Rectangle::new(Point::ORIGIN, Size::new(split_x, bounds.height)),
            |frame| {
                frame.draw_image(
                    before_bounds,
                    canvas::Image::new(self.preview.before.clone()),
                );
            },
        );
        frame.fill_rectangle(
            Point::new(split_x - 1.0, 0.0),
            Size::new(2.0, bounds.height),
            Color::from_rgb8(59, 130, 246),
        );
        frame.fill(
            &canvas::Path::circle(Point::new(split_x, bounds.height / 2.0), 8.0),
            Color::from_rgb8(59, 130, 246),
        );

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.dragging {
            mouse::Interaction::Grabbing
        } else if cursor.position_over(bounds).is_some() {
            mouse::Interaction::ResizingHorizontally
        } else {
            mouse::Interaction::default()
        }
    }
}

pub fn main() -> iced::Result {
    iced::application("Visual Diff", App::update, App::view)
        .theme(App::theme)
        .font(include_bytes!(env!("JETBRAINS_MONO_REGULAR")).as_slice())
        .font(include_bytes!(env!("JETBRAINS_MONO_MEDIUM")).as_slice())
        .window_size((1280.0, 800.0))
        .run_with(App::new)
}

#[derive(Debug)]
struct App {
    repo: Option<PathBuf>,
    recent_repos: Vec<PathBuf>,
    files: Vec<ChangedFile>,
    image_classifications: HashMap<PathBuf, ImageClassification>,
    classifying_images: bool,
    image_classification_generation: u64,
    show_pixel_identical: bool,
    selected: Option<usize>,
    versions: Option<(Vec<u8>, Vec<u8>)>,
    image_preview: Option<ImagePreview>,
    image_split: f32,
    text_layout: TextLayout,
    image_layout: ImageLayout,
    prepared_diff: Option<PreparedDiff>,
    diff_error: Option<String>,
    preparing_diff: bool,
    prepare_generation: u64,
    expanded_sections: HashSet<usize>,
    ignored_keys: String,
    prettify_json: bool,
    wrap_lines: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    ChooseRepository,
    RepositoryPicked(Option<PathBuf>),
    OpenRepository(PathBuf),
    Refresh,
    SelectFile(usize),
    IgnoredKeysChanged(String),
    PrettifyJsonChanged(bool),
    WrapLinesChanged(bool),
    BeginPreparation(u64),
    DiffPrepared(u64, Result<PreparedDiff, String>),
    ExpandSection(usize),
    ImagesClassified(u64, Result<HashMap<PathBuf, ImageClassification>, String>),
    ShowPixelIdenticalChanged(bool),
    ImageSplitChanged(f32),
    TextLayoutChanged(TextLayout),
    ImageLayoutChanged(ImageLayout),
}

impl App {
    fn new() -> (Self, Task<Message>) {
        (
            Self {
                repo: None,
                recent_repos: load_recent_repos(),
                files: Vec::new(),
                image_classifications: HashMap::new(),
                classifying_images: false,
                image_classification_generation: 0,
                show_pixel_identical: false,
                selected: None,
                versions: None,
                image_preview: None,
                image_split: 0.5,
                text_layout: TextLayout::SideBySide,
                image_layout: ImageLayout::Reveal,
                prepared_diff: None,
                diff_error: None,
                preparing_diff: false,
                prepare_generation: 0,
                expanded_sections: HashSet::new(),
                ignored_keys: String::new(),
                prettify_json: true,
                wrap_lines: true,
                error: None,
            },
            Task::none(),
        )
    }

    fn theme(&self) -> Theme {
        Theme::custom(
            "Visual Diff".to_owned(),
            iced::theme::Palette {
                background: Color::from_rgb8(9, 9, 11),
                text: Color::from_rgb8(244, 244, 245),
                primary: Color::from_rgb8(59, 130, 246),
                success: Color::from_rgb8(74, 222, 128),
                danger: Color::from_rgb8(248, 113, 113),
            },
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ChooseRepository => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .pick_folder()
                        .await
                        .map(|handle| handle.path().to_owned())
                },
                Message::RepositoryPicked,
            ),
            Message::RepositoryPicked(Some(path)) | Message::OpenRepository(path) => {
                self.open_repository(path)
            }
            Message::RepositoryPicked(None) => Task::none(),
            Message::Refresh => self.refresh(),
            Message::SelectFile(index) => self.select_file(index),
            Message::IgnoredKeysChanged(keys) => {
                self.ignored_keys = keys;
                self.schedule_preparation(true)
            }
            Message::PrettifyJsonChanged(prettify) => {
                self.prettify_json = prettify;
                self.schedule_preparation(false)
            }
            Message::WrapLinesChanged(wrap) => {
                self.wrap_lines = wrap;
                Task::none()
            }
            Message::BeginPreparation(generation) => {
                if generation == self.prepare_generation {
                    self.prepare_selected(generation)
                } else {
                    Task::none()
                }
            }
            Message::DiffPrepared(generation, result) => {
                if generation == self.prepare_generation {
                    self.preparing_diff = false;
                    match result {
                        Ok(diff) => {
                            self.prepared_diff = Some(diff);
                            self.diff_error = None;
                        }
                        Err(error) => {
                            self.prepared_diff = None;
                            self.diff_error = Some(error);
                        }
                    }
                }
                Task::none()
            }
            Message::ExpandSection(start) => {
                self.expanded_sections.insert(start);
                Task::none()
            }
            Message::ImagesClassified(generation, result) => {
                if generation != self.image_classification_generation {
                    return Task::none();
                }
                self.classifying_images = false;
                match result {
                    Ok(classifications) => self.image_classifications = classifications,
                    Err(error) => self.error = Some(error),
                }

                if self.selected.is_some_and(|index| !self.file_visible(index)) {
                    if let Some(index) = self.first_visible_file() {
                        return self.select_file(index);
                    }
                    self.selected = None;
                    self.versions = None;
                    self.image_preview = None;
                }
                Task::none()
            }
            Message::ShowPixelIdenticalChanged(show) => {
                self.show_pixel_identical = show;
                if self.selected.is_some_and(|index| !self.file_visible(index))
                    && let Some(index) = self.first_visible_file()
                {
                    return self.select_file(index);
                }
                Task::none()
            }
            Message::ImageSplitChanged(split) => {
                self.image_split = split.clamp(0.0, 1.0);
                Task::none()
            }
            Message::TextLayoutChanged(layout) => {
                self.text_layout = layout;
                Task::none()
            }
            Message::ImageLayoutChanged(layout) => {
                self.image_layout = layout;
                Task::none()
            }
        }
    }

    fn open_repository(&mut self, path: PathBuf) -> Task<Message> {
        match git::repository_root(&path) {
            Ok(root) => {
                self.repo = Some(root.clone());
                self.error = None;
                self.recent_repos.retain(|repo| repo != &root);
                self.recent_repos.insert(0, root);
                self.recent_repos.truncate(8);
                save_recent_repos(&self.recent_repos);
                self.refresh()
            }
            Err(error) => {
                self.error = Some(error);
                Task::none()
            }
        }
    }

    fn refresh(&mut self) -> Task<Message> {
        let Some(repo) = &self.repo else {
            return Task::none();
        };

        match git::changed_files(repo) {
            Ok(files) => {
                self.files = files;
                self.image_classifications.clear();
                self.image_classification_generation += 1;
                self.error = None;
                if self.files.is_empty() {
                    self.selected = None;
                    self.versions = None;
                    self.prepared_diff = None;
                    Task::none()
                } else {
                    let selected =
                        self.select_file(self.selected.unwrap_or(0).min(self.files.len() - 1));
                    Task::batch([selected, self.classify_images()])
                }
            }
            Err(error) => {
                self.error = Some(error);
                Task::none()
            }
        }
    }

    fn select_file(&mut self, index: usize) -> Task<Message> {
        let (Some(repo), Some(file)) = (&self.repo, self.files.get(index)) else {
            return Task::none();
        };

        match git::file_versions(repo, file) {
            Ok(versions) => {
                self.image_preview = if is_image(&file.path) {
                    image_preview(&versions.0, &versions.1)
                } else {
                    None
                };
                self.selected = Some(index);
                self.versions = Some(versions);
                self.error = None;
                self.prepared_diff = None;
                self.diff_error = None;
                self.expanded_sections.clear();
                self.schedule_preparation(false)
            }
            Err(error) => {
                self.error = Some(error);
                Task::none()
            }
        }
    }

    fn classify_images(&mut self) -> Task<Message> {
        let Some(repo) = self.repo.clone() else {
            return Task::none();
        };
        let files = self
            .files
            .iter()
            .filter(|file| is_classifiable_image(&file.path))
            .cloned()
            .collect::<Vec<_>>();
        if files.is_empty() {
            self.classifying_images = false;
            return Task::none();
        }

        self.classifying_images = true;
        self.image_classification_generation += 1;
        let generation = self.image_classification_generation;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || classify_images(&repo, &files))
                    .await
                    .map_err(|error| format!("Could not classify images: {error}"))?
            },
            move |result| Message::ImagesClassified(generation, result),
        )
    }

    fn file_visible(&self, index: usize) -> bool {
        self.show_pixel_identical
            || self.image_classifications.get(&self.files[index].path)
                != Some(&ImageClassification::PixelIdentical)
    }

    fn first_visible_file(&self) -> Option<usize> {
        (0..self.files.len()).find(|index| self.file_visible(*index))
    }

    fn schedule_preparation(&mut self, debounce: bool) -> Task<Message> {
        self.prepare_generation += 1;
        let generation = self.prepare_generation;
        self.expanded_sections.clear();

        if debounce {
            Task::perform(
                async { tokio::time::sleep(Duration::from_millis(300)).await },
                move |_| Message::BeginPreparation(generation),
            )
        } else {
            self.prepare_selected(generation)
        }
    }

    fn prepare_selected(&mut self, generation: u64) -> Task<Message> {
        let (Some(index), Some((before, after))) = (self.selected, &self.versions) else {
            return Task::none();
        };
        let file = &self.files[index];
        if is_image(&file.path) {
            self.preparing_diff = false;
            return Task::none();
        }

        self.preparing_diff = true;
        let before = before.clone();
        let after = after.clone();
        let is_json = is_json(&file.path);
        let ignored_keys = self.ignored_keys.clone();
        let prettify_json = self.prettify_json;

        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    prepare_diff(before, after, is_json, ignored_keys, prettify_json)
                })
                .await
                .map_err(|error| format!("Could not prepare diff: {error}"))?
            },
            move |result| Message::DiffPrepared(generation, result),
        )
    }

    fn view(&self) -> Element<'_, Message> {
        if self.repo.is_none() {
            return self.repository_picker();
        }

        let header = self.header();
        let workspace = row![self.file_sidebar(), self.diff_view()].height(Fill);
        let mut content = column![header, horizontal_rule(1), workspace];

        if let Some(error) = &self.error {
            content = content.push(error_banner(error));
        }

        container(content).width(Fill).height(Fill).into()
    }

    fn repository_picker(&self) -> Element<'_, Message> {
        let mut recent = column![text("Recent repositories").size(13)].spacing(8);

        if self.recent_repos.is_empty() {
            recent = recent.push(muted_text("No recent repositories"));
        } else {
            for repo in &self.recent_repos {
                recent = recent.push(
                    button(
                        row![
                            text(repo_name(repo)).size(14),
                            text(repo.display().to_string()).size(12).font(MONO)
                        ]
                        .spacing(12)
                        .align_y(Alignment::Center),
                    )
                    .on_press(Message::OpenRepository(repo.clone()))
                    .style(button::secondary)
                    .padding([10, 12])
                    .width(Fill),
                );
            }
        }

        let mut panel = column![
            text("Visual Diff").size(24),
            muted_text("Open a Git repository to inspect uncommitted changes."),
            button("Open repository")
                .on_press(Message::ChooseRepository)
                .padding([10, 16]),
            horizontal_rule(1),
            recent,
        ]
        .spacing(18)
        .width(560);

        if let Some(error) = &self.error {
            panel = panel.push(error_banner(error));
        }

        container(panel)
            .padding(32)
            .center_x(Fill)
            .center_y(Fill)
            .into()
    }

    fn header(&self) -> Element<'_, Message> {
        let repo = self.repo.as_ref().expect("repository is open");
        container(
            row![
                column![
                    text(repo_name(repo)).size(16),
                    text(repo.display().to_string()).size(11).font(MONO)
                ]
                .spacing(2)
                .width(Fill),
                button("Open another")
                    .on_press(Message::ChooseRepository)
                    .style(button::secondary),
                button("Refresh").on_press(Message::Refresh),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([12, 16])
        .into()
    }

    fn file_sidebar(&self) -> Element<'_, Message> {
        let visible_count = (0..self.files.len())
            .filter(|index| self.file_visible(*index))
            .count();
        let identical_count = self
            .image_classifications
            .values()
            .filter(|classification| **classification == ImageClassification::PixelIdentical)
            .count();
        let mut files = column![
            row![
                text("Changes").size(13).width(Fill),
                text(visible_count.to_string())
                    .size(13)
                    .color(Color::from_rgb8(161, 161, 170))
            ]
            .align_y(Alignment::Center),
            checkbox(
                format!("Show pixel-identical ({identical_count})"),
                self.show_pixel_identical
            )
            .on_toggle(Message::ShowPixelIdenticalChanged)
            .size(14)
        ]
        .spacing(6);

        if self.classifying_images {
            files = files.push(muted_text("Checking image pixels..."));
        }

        if visible_count == 0 {
            files = files.push(muted_text(if self.files.is_empty() {
                "Working tree is clean"
            } else {
                "All changes are pixel-identical"
            }));
        } else {
            for (index, file) in self.files.iter().enumerate() {
                if !self.file_visible(index) {
                    continue;
                }
                let selected = self.selected == Some(index);
                let mut label = row![
                    text(file.status.label())
                        .font(MONO)
                        .size(13)
                        .color(status_color(file.status)),
                    text(file.path.display().to_string())
                        .font(MONO)
                        .size(13)
                        .width(Fill),
                ]
                .spacing(10)
                .align_y(Alignment::Center);
                if self.image_classifications.get(&file.path)
                    == Some(&ImageClassification::PixelIdentical)
                {
                    label = label.push(
                        text("SAME")
                            .font(MONO)
                            .size(10)
                            .color(Color::from_rgb8(125, 211, 252)),
                    );
                }
                let item = button(label)
                    .on_press(Message::SelectFile(index))
                    .style(if selected {
                        button::primary
                    } else {
                        button::text
                    })
                    .padding([9, 10])
                    .width(Fill);
                files = files.push(item);
            }
        }

        container(scrollable(files).height(Fill))
            .width(SIDEBAR_WIDTH)
            .height(Fill)
            .padding(12)
            .style(panel_style)
            .into()
    }

    fn diff_view(&self) -> Element<'_, Message> {
        let (Some(index), Some((before, after))) = (self.selected, &self.versions) else {
            return container(muted_text("Select a changed file"))
                .center_x(Fill)
                .center_y(Fill)
                .into();
        };
        let file = &self.files[index];

        if is_image(&file.path) {
            return self.image_diff(file, before, after);
        }

        if is_video(&file.path) {
            return unsupported_file(file, "Video preview is not supported");
        }

        if is_json(&file.path) {
            self.json_diff(file)
        } else {
            self.text_diff(file, None, false)
        }
    }

    fn json_diff<'a>(&'a self, file: &'a ChangedFile) -> Element<'a, Message> {
        let controls = row![
            text_input("Ignored keys, comma-separated", &self.ignored_keys)
                .on_input(Message::IgnoredKeysChanged)
                .padding(8)
                .width(Fill),
            checkbox("Prettify JSON", self.prettify_json).on_toggle(Message::PrettifyJsonChanged),
        ]
        .spacing(16)
        .align_y(Alignment::Center)
        .width(Fill);

        self.text_diff(file, Some(controls.into()), true)
    }

    fn text_diff<'a>(
        &'a self,
        file: &'a ChangedFile,
        controls: Option<Element<'a, Message>>,
        highlight_json: bool,
    ) -> Element<'a, Message> {
        let mut content = column![file_header(file)];
        let mut toolbar = row![].spacing(16).align_y(Alignment::Center);
        if let Some(controls) = controls {
            toolbar = toolbar.push(controls);
        }
        toolbar = toolbar
            .push(checkbox("Wrap lines", self.wrap_lines).on_toggle(Message::WrapLinesChanged));
        if self.preparing_diff {
            toolbar = toolbar.push(muted_text("Updating..."));
        }
        let modes = row![
            button("Side by side")
                .on_press(Message::TextLayoutChanged(TextLayout::SideBySide))
                .style(if self.text_layout == TextLayout::SideBySide {
                    button::primary
                } else {
                    button::secondary
                }),
            button("Unified")
                .on_press(Message::TextLayoutChanged(TextLayout::Unified))
                .style(if self.text_layout == TextLayout::Unified {
                    button::primary
                } else {
                    button::secondary
                })
        ]
        .spacing(6);
        content = content.push(
            container(column![toolbar, modes].spacing(8))
                .padding([10, 16])
                .style(panel_style),
        );

        if let Some(error) = &self.diff_error {
            content = content.push(
                container(error_banner(error))
                    .padding(16)
                    .width(Fill)
                    .height(Fill),
            );
        } else if let Some(diff) = &self.prepared_diff {
            let mut lines = column![].spacing(0).width(Fill);
            for visible in visible_rows(&diff.rows, &self.expanded_sections) {
                match visible {
                    VisibleRow::Line(index) => {
                        let row_data = &diff.rows[index];
                        lines = if self.text_layout == TextLayout::Unified {
                            lines.push(diff_cell(
                                unified_cell(row_data),
                                highlight_json,
                                self.wrap_lines,
                                Fill,
                            ))
                        } else {
                            lines.push(
                                row![
                                    diff_cell(
                                        &row_data.left,
                                        highlight_json,
                                        self.wrap_lines,
                                        Length::FillPortion(1)
                                    ),
                                    diff_cell(
                                        &row_data.right,
                                        highlight_json,
                                        self.wrap_lines,
                                        Length::FillPortion(1)
                                    )
                                ]
                                .spacing(1)
                                .width(Fill),
                            )
                        };
                    }
                    VisibleRow::Collapsed { start, count } => {
                        lines = lines.push(
                            button(text(format!("Show {count} unchanged lines")).size(12))
                                .on_press(Message::ExpandSection(start))
                                .style(button::secondary)
                                .padding([7, 12])
                                .width(Fill),
                        );
                    }
                }
            }

            content = content.push(
                column![
                    if self.text_layout == TextLayout::Unified {
                        row![pane_title("Unified")].width(Fill)
                    } else {
                        row![pane_title("HEAD"), pane_title("Working tree")]
                            .spacing(1)
                            .width(Fill)
                    },
                    scrollable(lines)
                        .direction(if self.wrap_lines {
                            scrollable::Direction::Vertical(scrollable::Scrollbar::default())
                        } else {
                            scrollable::Direction::Both {
                                vertical: scrollable::Scrollbar::default(),
                                horizontal: scrollable::Scrollbar::default(),
                            }
                        })
                        .width(Fill)
                        .height(Fill)
                ]
                .height(Fill),
            );
        } else {
            content = content.push(
                container(muted_text("Preparing diff..."))
                    .center_x(Fill)
                    .center_y(Fill),
            );
        }

        container(content).width(Fill).height(Fill).into()
    }

    fn image_diff<'a>(
        &'a self,
        file: &'a ChangedFile,
        before: &'a [u8],
        after: &'a [u8],
    ) -> Element<'a, Message> {
        let comparison: Element<'_, Message> = if self.image_layout == ImageLayout::Reveal
            && let Some(preview) = &self.image_preview
        {
            column![
                row![
                    text("HEAD").font(MONO).size(11).width(Fill),
                    text("Working tree").font(MONO).size(11)
                ]
                .padding([8, 12]),
                container(
                    canvas(ImageReveal {
                        preview: preview.clone(),
                        split: self.image_split,
                    })
                    .width(Fill)
                    .height(Fill)
                )
                .padding(16)
                .width(Fill)
                .height(Fill)
                .style(checker_style),
                slider(0.0..=1.0, self.image_split, Message::ImageSplitChanged).step(0.01_f32)
            ]
            .spacing(8)
            .padding(Padding {
                top: 0.0,
                right: 16.0,
                bottom: 16.0,
                left: 16.0,
            })
            .into()
        } else {
            row![
                image_pane("HEAD", before),
                image_pane("Working tree", after)
            ]
            .spacing(1)
            .height(Fill)
            .into()
        };

        let modes = row![
            button("Reveal")
                .on_press(Message::ImageLayoutChanged(ImageLayout::Reveal))
                .style(if self.image_layout == ImageLayout::Reveal {
                    button::primary
                } else {
                    button::secondary
                }),
            button("Side by side")
                .on_press(Message::ImageLayoutChanged(ImageLayout::SideBySide))
                .style(if self.image_layout == ImageLayout::SideBySide {
                    button::primary
                } else {
                    button::secondary
                })
        ]
        .spacing(6);

        container(column![
            file_header(file),
            container(modes).padding([8, 16]).style(panel_style),
            comparison
        ])
        .width(Fill)
        .height(Fill)
        .into()
    }
}

fn file_header(file: &ChangedFile) -> Element<'_, Message> {
    container(
        row![
            text(file.path.display().to_string())
                .font(MONO)
                .size(13)
                .width(Fill),
            text(file.status.label())
                .font(MONO)
                .color(status_color(file.status)),
        ]
        .align_y(Alignment::Center),
    )
    .padding([12, 16])
    .style(panel_style)
    .into()
}

fn unsupported_file<'a>(file: &'a ChangedFile, message: &'a str) -> Element<'a, Message> {
    container(
        column![file_header(file), muted_text(message)]
            .spacing(16)
            .width(Fill),
    )
    .width(Fill)
    .height(Fill)
    .padding(16)
    .into()
}

fn image_pane<'a>(label: &'a str, bytes: &'a [u8]) -> Element<'a, Message> {
    let preview: Element<'_, Message> = if bytes.is_empty() {
        container(muted_text("No image"))
            .center_x(Fill)
            .center_y(Fill)
            .into()
    } else {
        container(
            image(image::Handle::from_bytes(bytes.to_vec()))
                .content_fit(iced::ContentFit::Contain)
                .width(Fill)
                .height(Fill),
        )
        .padding(24)
        .center_x(Fill)
        .center_y(Fill)
        .into()
    };

    container(column![pane_title(label), preview])
        .width(Length::FillPortion(1))
        .height(Fill)
        .style(checker_style)
        .into()
}

fn split_message(cursor: mouse::Cursor, bounds: Rectangle) -> Option<Message> {
    cursor.position().map(|position| {
        Message::ImageSplitChanged(((position.x - bounds.x) / bounds.width).clamp(0.0, 1.0))
    })
}

fn contain_bounds(image_size: Size<u32>, bounds: Rectangle) -> Rectangle {
    let image_size = Size::new(image_size.width as f32, image_size.height as f32);
    let scale = (bounds.width / image_size.width)
        .min(bounds.height / image_size.height)
        .max(0.0);
    let size = image_size * scale;
    Rectangle::new(
        Point::new(
            bounds.x + (bounds.width - size.width) / 2.0,
            bounds.y + (bounds.height - size.height) / 2.0,
        ),
        size,
    )
}

fn image_preview(before: &[u8], after: &[u8]) -> Option<ImagePreview> {
    if before.is_empty() || after.is_empty() {
        return None;
    }
    let before_size = ::image::load_from_memory(before).ok()?.dimensions();
    let after_size = ::image::load_from_memory(after).ok()?.dimensions();
    Some(ImagePreview {
        before: image::Handle::from_bytes(before.to_vec()),
        after: image::Handle::from_bytes(after.to_vec()),
        before_size: Size::new(before_size.0, before_size.1),
        after_size: Size::new(after_size.0, after_size.1),
    })
}

fn classify_images(
    repo: &Path,
    files: &[ChangedFile],
) -> Result<HashMap<PathBuf, ImageClassification>, String> {
    let mut classifications = HashMap::new();
    git::visit_file_versions(repo, files, |file, before, after| {
        classifications.insert(file.path.clone(), classify_image(before, after));
    })?;
    Ok(classifications)
}

fn classify_image(before: &[u8], after: &[u8]) -> ImageClassification {
    if before.is_empty() || after.is_empty() {
        return ImageClassification::VisuallyChanged;
    }
    let (Ok(before), Ok(after)) = (
        ::image::load_from_memory(before),
        ::image::load_from_memory(after),
    ) else {
        return ImageClassification::VisuallyChanged;
    };
    if before.dimensions() == after.dimensions() && before.to_rgba8() == after.to_rgba8() {
        ImageClassification::PixelIdentical
    } else {
        ImageClassification::VisuallyChanged
    }
}

fn unified_cell(row: &DiffRow) -> &DiffCell {
    if row.left.kind == DiffKind::Removed {
        &row.left
    } else if row.right.kind == DiffKind::Added {
        &row.right
    } else {
        &row.left
    }
}

fn diff_cell<'a>(
    cell: &'a DiffCell,
    highlight_json: bool,
    wrap: bool,
    width: Length,
) -> Element<'a, Message> {
    let wrapping = if wrap {
        text::Wrapping::WordOrGlyph
    } else {
        text::Wrapping::None
    };
    let line = if cell.line.is_empty() {
        " "
    } else {
        &cell.line
    };
    let content: Element<'_, Message> = if highlight_json {
        rich_text(json_spans(&cell.tokens, line))
            .font(MONO)
            .size(CODE_SIZE)
            .line_height(text::LineHeight::Relative(1.45))
            .wrapping(wrapping)
            .into()
    } else {
        text(line.to_owned())
            .font(MONO)
            .size(CODE_SIZE)
            .line_height(text::LineHeight::Relative(1.45))
            .wrapping(wrapping)
            .into()
    };

    container(
        row![
            text(cell.marker)
                .font(MONO)
                .size(CODE_SIZE)
                .line_height(text::LineHeight::Relative(1.45))
                .width(20),
            content
        ]
        .padding(Padding::from([5, 10])),
    )
    .width(width)
    .style(match cell.kind {
        DiffKind::Added => added_style,
        DiffKind::Removed => removed_style,
        DiffKind::Unchanged => unchanged_style,
    })
    .into()
}

fn pane_title(label: &str) -> Element<'_, Message> {
    container(text(label).size(11).font(MONO))
        .padding([8, 10])
        .width(Length::FillPortion(1))
        .style(panel_style)
        .into()
}

fn muted_text(value: &str) -> iced::widget::Text<'_> {
    text(value).size(13).color(Color::from_rgb8(161, 161, 170))
}

fn error_banner(error: impl ToString) -> Element<'static, Message> {
    container(
        text(error.to_string())
            .size(13)
            .color(Color::from_rgb8(254, 202, 202)),
    )
    .padding([10, 12])
    .width(Fill)
    .style(|_| container::Style {
        background: Some(Color::from_rgb8(69, 10, 10).into()),
        border: Border::default().rounded(6),
        ..Default::default()
    })
    .into()
}

fn panel_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb8(24, 24, 27).into()),
        border: Border {
            color: Color::from_rgb8(63, 63, 70),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn checker_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb8(18, 18, 21).into()),
        ..Default::default()
    }
}

fn removed_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb8(69, 10, 10).into()),
        ..Default::default()
    }
}

fn added_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb8(5, 46, 22).into()),
        ..Default::default()
    }
}

fn unchanged_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb8(9, 9, 11).into()),
        ..Default::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonToken {
    Plain,
    Key,
    String,
    Number,
    Boolean,
    Null,
}

fn prepare_diff(
    before: Vec<u8>,
    after: Vec<u8>,
    highlight_json: bool,
    ignored_keys: String,
    prettify_json: bool,
) -> Result<PreparedDiff, String> {
    let (before, after) = if highlight_json {
        prepare_json(&before, &after, &ignored_keys, prettify_json)?
    } else {
        (
            String::from_utf8(before)
                .map_err(|_| "Binary preview is not supported for this file type".to_owned())?,
            String::from_utf8(after)
                .map_err(|_| "Binary preview is not supported for this file type".to_owned())?,
        )
    };
    let diff = TextDiff::from_lines(&before, &after);
    let mut rows = Vec::new();

    for change in diff.iter_all_changes() {
        let line = change.value().trim_end_matches('\n');
        let (left, right, changed) = match change.tag() {
            ChangeTag::Delete => (
                diff_cell_data("-", line, DiffKind::Removed, highlight_json),
                diff_cell_data("", "", DiffKind::Unchanged, highlight_json),
                true,
            ),
            ChangeTag::Insert => (
                diff_cell_data("", "", DiffKind::Unchanged, highlight_json),
                diff_cell_data("+", line, DiffKind::Added, highlight_json),
                true,
            ),
            ChangeTag::Equal => {
                let left = diff_cell_data(" ", line, DiffKind::Unchanged, highlight_json);
                (left.clone(), left, false)
            }
        };
        rows.push(DiffRow {
            left,
            right,
            changed,
        });
    }

    Ok(PreparedDiff { rows })
}

fn diff_cell_data(
    marker: &'static str,
    line: &str,
    kind: DiffKind,
    highlight_json: bool,
) -> DiffCell {
    DiffCell {
        marker,
        line: line.to_owned(),
        kind,
        tokens: if highlight_json && !line.is_empty() {
            json_tokens(line)
        } else {
            Vec::new()
        },
    }
}

fn visible_rows(rows: &[DiffRow], expanded: &HashSet<usize>) -> Vec<VisibleRow> {
    let mut visible = Vec::new();
    let mut index = 0;

    while index < rows.len() {
        if rows[index].changed {
            visible.push(VisibleRow::Line(index));
            index += 1;
            continue;
        }

        let start = index;
        while index < rows.len() && !rows[index].changed {
            index += 1;
        }
        let end = index;
        let leading = if start == 0 && end < rows.len() {
            0
        } else {
            CONTEXT_LINES.min(end - start)
        };
        let trailing = if end == rows.len() && start > 0 {
            0
        } else {
            CONTEXT_LINES.min(end - start - leading)
        };

        if expanded.contains(&start) || end - start <= leading + trailing + 1 {
            visible.extend((start..end).map(VisibleRow::Line));
            continue;
        }

        visible.extend((start..start + leading).map(VisibleRow::Line));
        visible.push(VisibleRow::Collapsed {
            start,
            count: end - start - leading - trailing,
        });
        visible.extend((end - trailing..end).map(VisibleRow::Line));
    }

    visible
}

fn json_spans<'a>(
    tokens: &'a [(String, JsonToken)],
    fallback: &'a str,
) -> Vec<text::Span<'a, Message>> {
    if tokens.is_empty() {
        return vec![span(fallback)];
    }

    tokens
        .iter()
        .map(|(value, token)| {
            let color = match token {
                JsonToken::Plain => Color::from_rgb8(212, 212, 216),
                JsonToken::Key => Color::from_rgb8(125, 211, 252),
                JsonToken::String => Color::from_rgb8(134, 239, 172),
                JsonToken::Number => Color::from_rgb8(253, 186, 116),
                JsonToken::Boolean => Color::from_rgb8(196, 181, 253),
                JsonToken::Null => Color::from_rgb8(251, 113, 133),
            };
            span(value.as_str()).color(color)
        })
        .collect()
}

fn json_tokens(line: &str) -> Vec<(String, JsonToken)> {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'"' {
            let mut end = index + 1;
            let mut escaped = false;
            while end < bytes.len() {
                if bytes[end] == b'"' && !escaped {
                    end += 1;
                    break;
                }
                escaped = bytes[end] == b'\\' && !escaped;
                if bytes[end] != b'\\' {
                    escaped = false;
                }
                end += 1;
            }
            let kind = if line[end..].trim_start().starts_with(':') {
                JsonToken::Key
            } else {
                JsonToken::String
            };
            push_json_token(&mut tokens, &line[index..end], kind);
            index = end;
            continue;
        }

        if bytes[index].is_ascii_digit()
            || (bytes[index] == b'-' && bytes.get(index + 1).is_some_and(u8::is_ascii_digit))
        {
            let mut end = index + 1;
            while end < bytes.len()
                && (bytes[end].is_ascii_digit()
                    || matches!(bytes[end], b'.' | b'e' | b'E' | b'+' | b'-'))
            {
                end += 1;
            }
            push_json_token(&mut tokens, &line[index..end], JsonToken::Number);
            index = end;
            continue;
        }

        let keyword = [
            ("true", JsonToken::Boolean),
            ("false", JsonToken::Boolean),
            ("null", JsonToken::Null),
        ]
        .into_iter()
        .find(|(keyword, _)| line[index..].starts_with(keyword));
        if let Some((keyword, kind)) = keyword {
            push_json_token(&mut tokens, keyword, kind);
            index += keyword.len();
            continue;
        }

        let character = line[index..]
            .chars()
            .next()
            .expect("valid character boundary");
        push_json_token(
            &mut tokens,
            &line[index..index + character.len_utf8()],
            JsonToken::Plain,
        );
        index += character.len_utf8();
    }

    tokens
}

fn push_json_token(tokens: &mut Vec<(String, JsonToken)>, value: &str, kind: JsonToken) {
    if let Some((previous, previous_kind)) = tokens.last_mut()
        && *previous_kind == kind
    {
        previous.push_str(value);
    } else {
        tokens.push((value.to_owned(), kind));
    }
}

fn status_color(status: FileStatus) -> Color {
    match status {
        FileStatus::Added | FileStatus::Untracked => Color::from_rgb8(74, 222, 128),
        FileStatus::Deleted => Color::from_rgb8(248, 113, 113),
        FileStatus::Modified | FileStatus::Renamed => Color::from_rgb8(250, 204, 21),
    }
}

fn repo_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp")
    )
}

fn is_video(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("avi" | "m4v" | "mkv" | "mov" | "mp4" | "mpeg" | "mpg" | "webm")
    )
}

fn is_classifiable_image(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("png" | "jpg" | "jpeg" | "bmp")
    )
}

fn is_json(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("json")
}

fn prepare_json(
    before: &[u8],
    after: &[u8],
    ignored_keys: &str,
    pretty: bool,
) -> Result<(String, String), String> {
    let ignored: HashSet<&str> = ignored_keys
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .collect();

    let prepare = |bytes: &[u8]| -> Result<String, String> {
        if bytes.is_empty() {
            return Ok(String::new());
        }
        if ignored.is_empty() && !pretty {
            return String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string());
        }

        let mut value: Value =
            serde_json::from_slice(bytes).map_err(|error| format!("Invalid JSON: {error}"))?;
        remove_keys(&mut value, &ignored);
        if pretty {
            serde_json::to_string_pretty(&value)
        } else {
            serde_json::to_string(&value)
        }
        .map_err(|error| error.to_string())
    };

    Ok((prepare(before)?, prepare(after)?))
}

fn remove_keys(value: &mut Value, ignored: &HashSet<&str>) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| !ignored.contains(key.as_str()));
            for value in object.values_mut() {
                remove_keys(value, ignored);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_keys(value, ignored);
            }
        }
        _ => {}
    }
}

fn recent_repos_path() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|config| config.join("visual-diff/recent-repos"))
}

fn load_recent_repos() -> Vec<PathBuf> {
    recent_repos_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|contents| {
            contents
                .lines()
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

fn save_recent_repos(repos: &[PathBuf]) {
    let Some(path) = recent_repos_path() else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_ok() {
        let contents = repos
            .iter()
            .map(|repo| repo.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let _ = fs::write(path, contents);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::image::{ColorType, ImageEncoder, codecs::png};

    fn encode_png(pixels: &[u8], compression: png::CompressionType) -> Vec<u8> {
        let mut bytes = Vec::new();
        png::PngEncoder::new_with_quality(&mut bytes, compression, png::FilterType::Adaptive)
            .write_image(pixels, 2, 2, ColorType::Rgba8)
            .unwrap();
        bytes
    }

    #[test]
    fn removes_ignored_json_keys_at_every_depth() {
        let before = br#"{"id":1,"meta":{"id":2},"items":[{"id":3,"name":"a"}]}"#;
        let (prepared, _) = prepare_json(before, b"", "id", false).unwrap();

        assert_eq!(prepared, r#"{"items":[{"name":"a"}],"meta":{}}"#);
    }

    #[test]
    fn preserves_raw_json_when_no_options_apply() {
        let raw = b"{\n  \"b\": 2, \"a\": 1\n}\n";
        let (prepared, _) = prepare_json(raw, b"", "", false).unwrap();

        assert_eq!(prepared.as_bytes(), raw);
    }

    #[test]
    fn classifies_json_syntax() {
        let tokens = json_tokens(r#"  "name": "A\"B", "count": -12.5, "ok": true, "value": null"#)
            .into_iter()
            .filter(|(_, kind)| *kind != JsonToken::Plain)
            .map(|(_, kind)| kind)
            .collect::<Vec<_>>();

        assert_eq!(
            tokens,
            vec![
                JsonToken::Key,
                JsonToken::String,
                JsonToken::Key,
                JsonToken::Number,
                JsonToken::Key,
                JsonToken::Boolean,
                JsonToken::Key,
                JsonToken::Null,
            ]
        );
    }

    #[test]
    fn prepares_and_caches_json_tokens() {
        let diff = prepare_diff(
            br#"{"count":1}"#.to_vec(),
            br#"{"count":2}"#.to_vec(),
            true,
            String::new(),
            false,
        )
        .unwrap();

        assert!(
            diff.rows
                .iter()
                .flat_map(|row| [&row.left, &row.right])
                .filter(|cell| !cell.line.is_empty())
                .all(|cell| !cell.tokens.is_empty())
        );
    }

    #[test]
    fn collapses_large_unchanged_sections() {
        let unchanged = || DiffRow {
            left: diff_cell_data(" ", "same", DiffKind::Unchanged, false),
            right: diff_cell_data(" ", "same", DiffKind::Unchanged, false),
            changed: false,
        };
        let changed = || DiffRow {
            left: diff_cell_data("-", "before", DiffKind::Removed, false),
            right: diff_cell_data("+", "after", DiffKind::Added, false),
            changed: true,
        };
        let mut rows = vec![changed()];
        rows.extend((0..100_000).map(|_| unchanged()));
        rows.push(changed());

        let visible = visible_rows(&rows, &HashSet::new());

        assert_eq!(visible.len(), 9);
        assert_eq!(
            visible[4],
            VisibleRow::Collapsed {
                start: 1,
                count: 99_994,
            }
        );
    }

    #[test]
    fn classifies_reencoded_pixels_as_identical() {
        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 0,
        ];
        let before = encode_png(&pixels, png::CompressionType::Fast);
        let after = encode_png(&pixels, png::CompressionType::Best);

        assert_ne!(before, after);
        assert_eq!(
            classify_image(&before, &after),
            ImageClassification::PixelIdentical
        );
    }

    #[test]
    fn classifies_pixel_edits_as_visual_changes() {
        let before = encode_png(&[0; 16], png::CompressionType::Fast);
        let mut pixels = [0; 16];
        pixels[0] = 1;
        let after = encode_png(&pixels, png::CompressionType::Fast);

        assert_eq!(
            classify_image(&before, &after),
            ImageClassification::VisuallyChanged
        );
    }

    #[test]
    fn selects_the_changed_cell_for_unified_rows() {
        let diff = prepare_diff(
            b"same\nbefore\n".to_vec(),
            b"same\nafter\n".to_vec(),
            false,
            String::new(),
            false,
        )
        .unwrap();
        let markers = diff
            .rows
            .iter()
            .map(|row| unified_cell(row).marker)
            .collect::<Vec<_>>();

        assert_eq!(markers, vec![" ", "-", "+"]);
    }

    #[test]
    fn recognizes_common_video_extensions() {
        assert!(is_video(Path::new("clip.mp4")));
        assert!(is_video(Path::new("clip.webm")));
        assert!(!is_video(Path::new("notes.txt")));
    }
}
