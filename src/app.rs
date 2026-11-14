//! KaraokeKit desktop application shell.

use crate::ass;
use crate::audio::{AudioEngine, Track};
use crate::export::{self, RenderEvent, RenderHandle};
use crate::lrc;
use crate::lyrics;
use crate::model::{AspectRatio, AudioCodec, Background, Container, Project};
use crate::preview;
use crate::theme;
use crate::timeline;
use crate::util::{self, format_time};
use eframe::egui;
use egui::{
    Align, Align2, Button, Color32, ComboBox, FontFamily, Label, Layout, ProgressBar, RichText,
    ScrollArea, Slider, Stroke, TextEdit, Ui, Vec2,
};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

const FONT_FAMILY: &str = "Karaoke";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stem {
    Instrumental,
    Vocal,
}

struct LoadMessage(Stem, Result<Track, String>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SideTab {
    Style,
    Export,
}

/// Destructive action waiting for an unsaved-changes decision.
#[derive(Clone)]
enum PendingAction {
    New,
    OpenDialog,
    OpenPath(PathBuf),
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusKind {
    Info,
    Good,
    Warn,
    Error,
}

enum FfmpegStatus {
    Checking,
    Ready(String),
    Missing(String),
}

struct ActiveRender {
    handle: RenderHandle,
    progress: f32,
    speed: Option<String>,
    cancelling: bool,
    started: Instant,
}

enum TokenRow {
    Header(usize),
    Token {
        flat: usize,
        line: usize,
        index: usize,
    },
}

pub struct KaraokeKitApp {
    project: Project,
    project_path: Option<PathBuf>,
    lyrics_draft: String,
    lyrics_window: bool,
    focus_lyrics_editor: bool,
    help_window: bool,
    error_dialog: Option<String>,
    /// Fingerprint of the project as last saved/loaded, used to warn before
    /// discarding edits on New/Open/close.
    saved_revision: u64,
    pending_action: Option<PendingAction>,
    confirmed_close: bool,

    instrument: Option<Arc<Track>>,
    vocal: Option<Arc<Track>>,
    engine: Option<AudioEngine>,
    engine_error: Option<String>,
    load_rx: Option<Receiver<LoadMessage>>,
    load_queue: VecDeque<(Stem, PathBuf)>,
    load_label: Option<String>,

    inst_gain: f32,
    vocal_gain: f32,
    mute_inst: bool,
    mute_vocal: bool,

    wave_tex: Option<egui::TextureHandle>,
    cover_tex: Option<egui::TextureHandle>,
    cover_key: Option<String>,
    bg_thumb: Option<egui::TextureHandle>,
    bg_thumb_key: Option<String>,

    selected: Option<usize>,
    tap_index: usize,
    tap_mode: bool,
    follow_tap: bool,
    scroll_to_selected: bool,
    manual_position: u64,

    tab: SideTab,
    export_path: Option<PathBuf>,
    render: Option<ActiveRender>,
    last_render: Option<(String, bool)>,

    ffmpeg_status: FfmpegStatus,
    ffmpeg_rx: Option<Receiver<Result<String, String>>>,

    status: String,
    status_kind: StatusKind,
    window_title: String,
}

impl KaraokeKitApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        install_fonts(&cc.egui_ctx);

        let (ffmpeg_tx, ffmpeg_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = ffmpeg_tx.send(util::probe_ffmpeg());
        });

        let project = Project::default();
        let saved_revision = Self::project_revision(&project);

        Self {
            project,
            project_path: None,
            lyrics_draft: String::new(),
            lyrics_window: false,
            focus_lyrics_editor: false,
            help_window: false,
            error_dialog: None,
            saved_revision,
            pending_action: None,
            confirmed_close: false,
            instrument: None,
            vocal: None,
            engine: None,
            engine_error: None,
            load_rx: None,
            load_queue: VecDeque::new(),
            load_label: None,
            inst_gain: 1.0,
            vocal_gain: 0.8,
            mute_inst: false,
            mute_vocal: false,
            wave_tex: None,
            cover_tex: None,
            cover_key: None,
            bg_thumb: None,
            bg_thumb_key: None,
            selected: None,
            tap_index: 0,
            tap_mode: true,
            follow_tap: true,
            scroll_to_selected: false,
            manual_position: 0,
            tab: SideTab::Style,
            export_path: None,
            render: None,
            last_render: None,
            ffmpeg_status: FfmpegStatus::Checking,
            ffmpeg_rx: Some(ffmpeg_rx),
            status: "Paste lyrics or open a project to begin.".to_owned(),
            status_kind: StatusKind::Info,
            window_title: String::new(),
        }
    }

    // ----- actions -------------------------------------------------------

    fn set_status(&mut self, kind: StatusKind, message: impl Into<String>) {
        self.status = message.into();
        self.status_kind = kind;
    }

    // ----- unsaved-changes guard ----------------------------------------

    /// Cheap-to-store fingerprint of everything that gets serialized.
    fn project_revision(project: &Project) -> u64 {
        use std::hash::{Hash, Hasher};
        let json = project.to_json().unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        json.hash(&mut hasher);
        hasher.finish()
    }

    fn is_dirty(&self) -> bool {
        Self::project_revision(&self.project) != self.saved_revision
    }

    fn mark_saved(&mut self) {
        self.saved_revision = Self::project_revision(&self.project);
    }

    fn request_new(&mut self) {
        if self.is_dirty() {
            self.pending_action = Some(PendingAction::New);
        } else {
            self.new_project();
        }
    }

    fn request_open_dialog(&mut self) {
        if self.is_dirty() {
            self.pending_action = Some(PendingAction::OpenDialog);
        } else {
            self.open_project_dialog();
        }
    }

    fn request_open_path(&mut self, path: PathBuf) {
        if self.is_dirty() {
            self.pending_action = Some(PendingAction::OpenPath(path));
        } else {
            self.open_path(path);
        }
    }

    /// Run a previously confirmed action, bypassing the dirty check so a
    /// deliberate "Discard changes" is not prompted for again.
    fn perform_pending(&mut self, action: PendingAction, ctx: &egui::Context) {
        match action {
            PendingAction::New => self.new_project(),
            PendingAction::OpenDialog => self.choose_and_open_project(),
            PendingAction::OpenPath(path) => self.open_path(path),
            PendingAction::Close => {
                self.confirmed_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn current_position(&self) -> u64 {
        self.engine
            .as_ref()
            .map(|e| e.position_ms())
            .unwrap_or(self.manual_position)
    }

    fn audio_duration(&self) -> u64 {
        self.instrument
            .as_ref()
            .or(self.vocal.as_ref())
            .map(|t| t.duration_ms)
            .unwrap_or(0)
            .saturating_add(self.project.preroll_ms())
    }

    fn timeline_duration(&self) -> u64 {
        let lyric_end = self
            .project
            .spans()
            .into_iter()
            .flatten()
            .map(|(_, e)| e)
            .max()
            .unwrap_or(0);
        self.audio_duration().max(lyric_end + 500).max(5_000)
    }

    fn seek(&mut self, ms: u64) {
        let ms = ms.min(self.timeline_duration());
        self.manual_position = ms;
        if let Some(engine) = &self.engine {
            engine.seek(ms);
        }
    }

    fn seek_relative(&mut self, delta_ms: i64) {
        let now = self.current_position() as i64 + delta_ms;
        self.seek(now.max(0) as u64);
    }

    fn toggle_play(&mut self) {
        if self.engine.is_some() {
            if let Some(engine) = &self.engine {
                engine.toggle();
            }
        } else if self.project.instrumental.is_some() {
            self.set_status(
                StatusKind::Warn,
                "The stem is still decoding — playback will be available in a moment.",
            );
        } else {
            self.set_status(StatusKind::Warn, "Load an instrumental track first.");
        }
    }

    fn start_tapping(&mut self) {
        if self.project.token_count() == 0 {
            self.set_status(StatusKind::Warn, "Paste and tokenize lyrics first.");
            return;
        }
        let seek_to = self
            .next_untimed()
            .and_then(|i| self.project.token_span(i))
            .map(|(start, _)| start.saturating_sub(1_500))
            .unwrap_or_else(|| self.current_position());
        self.seek(seek_to);
        let has_engine = self.engine.is_some();
        if let Some(engine) = &self.engine {
            engine.play();
        }
        if has_engine {
            self.set_status(
                StatusKind::Good,
                "Playing — tap Space on every syllable, Backspace to undo.",
            );
        } else {
            self.set_status(
                StatusKind::Warn,
                "Tap mode is armed, but no audio output is available.",
            );
        }
        self.tap_mode = true;
        self.scroll_to_selected = true;
    }

    fn next_untimed(&self) -> Option<usize> {
        if self.tap_index < self.project.token_count()
            && self
                .project
                .token(self.tap_index)
                .map(|t| t.start_ms.is_none())
                .unwrap_or(false)
        {
            Some(self.tap_index)
        } else {
            self.project.first_untimed()
        }
    }

    fn tap(&mut self) {
        if !self.tap_mode {
            self.toggle_play();
            return;
        }
        let Some(index) = self.next_untimed() else {
            self.set_status(StatusKind::Good, "Every token has a timestamp.");
            if let Some(engine) = &self.engine {
                engine.pause();
            }
            return;
        };
        let timeline_position = self.current_position();
        if timeline_position < self.project.preroll_ms() {
            self.set_status(
                StatusKind::Warn,
                "Title-card pre-roll — wait for the audio to start before tapping.",
            );
            return;
        }
        let position = timeline_position.saturating_sub(self.project.preroll_ms());
        // Keep the raw starts non-decreasing even if the user taps slightly out of order.
        let position = if index > 0 {
            let previous_raw = self
                .project
                .token(index - 1)
                .and_then(|t| t.start_ms)
                .unwrap_or(0);
            position.max(previous_raw)
        } else {
            position
        };
        self.project.set_token_start(index, position);
        self.selected = Some(index);
        self.tap_index = index + 1;
        self.scroll_to_selected = true;
        let text = self
            .project
            .token(index)
            .map(|t| t.text.clone())
            .unwrap_or_default();
        self.set_status(
            StatusKind::Info,
            format!(
                "Tapped “{text}” at {} ({}/{})",
                format_time(timeline_position),
                index + 1,
                self.project.token_count()
            ),
        );
    }

    fn undo_tap(&mut self, seek_back: bool) {
        if self.tap_index == 0 {
            self.set_status(StatusKind::Info, "Nothing to undo.");
            return;
        }
        let index = self.tap_index - 1;
        let previous_start = self.project.token_span(index).map(|(start, _)| start);
        self.project.clear_token_start(index);
        self.tap_index = index;
        self.selected = Some(index);
        self.scroll_to_selected = true;
        if seek_back {
            if let Some(start) = previous_start {
                self.seek(start.saturating_sub(150));
            }
        }
        self.set_status(
            StatusKind::Warn,
            format!(
                "Re-tapping token {} — press Space on the syllable.",
                index + 1
            ),
        );
    }

    fn nudge_selected(&mut self, delta: i64) {
        if let Some(index) = self.selected {
            self.project.nudge_token(index, delta);
            self.set_status(
                StatusKind::Info,
                format!("Token {} nudged {:+.0} ms", index + 1, delta as f32),
            );
        } else {
            self.set_status(StatusKind::Info, "Select a token to nudge it.");
        }
    }

    fn set_selected_to_playhead(&mut self) {
        if let Some(index) = self.selected {
            let timeline_position = self.current_position();
            if timeline_position < self.project.preroll_ms() {
                self.set_status(
                    StatusKind::Warn,
                    "Title-card pre-roll — wait for the audio to start.",
                );
                return;
            }
            let position = timeline_position.saturating_sub(self.project.preroll_ms());
            self.project.set_token_start(index, position);
            self.tap_index = index + 1;
            self.scroll_to_selected = true;
            let text = self
                .project
                .token(index)
                .map(|t| t.text.clone())
                .unwrap_or_default();
            self.set_status(
                StatusKind::Good,
                format!("“{text}” set to {}", format_time(timeline_position)),
            );
        }
    }

    fn apply_lyrics(&mut self) -> bool {
        let lines = lyrics::tokenize(&self.lyrics_draft);
        if lines.is_empty() {
            self.set_status(StatusKind::Error, "No lyric lines found in the editor.");
            return false;
        }
        let token_count: usize = lines.iter().map(|l| l.tokens.len()).sum();
        self.project.lines = lines;
        self.selected = None;
        self.tap_index = 0;
        self.scroll_to_selected = true;
        self.set_status(
            StatusKind::Good,
            format!(
                "Tokenized {} lines / {} syllables. Timings were reset.",
                self.project.lines.len(),
                token_count
            ),
        );
        true
    }

    fn sync_lyrics_draft(&mut self) {
        self.lyrics_draft = self
            .project
            .lines
            .iter()
            .map(|line| lyrics::join_tokens_with_slashes(&line.tokens))
            .collect::<Vec<_>>()
            .join("\n");
    }

    // ----- loading -------------------------------------------------------

    fn start_dir(&self) -> PathBuf {
        self.project_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_owned)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn enqueue_stem(&mut self, kind: Stem, path: PathBuf) {
        self.load_queue.push_back((kind, path));
        self.start_next_load();
    }

    fn start_next_load(&mut self) {
        if self.load_rx.is_some() {
            return;
        }
        let Some((kind, path)) = self.load_queue.pop_front() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.load_rx = Some(rx);
        self.load_label = Some(format!(
            "Decoding {}…",
            path.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        ));
        std::thread::spawn(move || {
            let _ = tx.send(LoadMessage(kind, crate::audio::decode_track(&path)));
        });
    }

    fn poll_load(&mut self, ctx: &egui::Context) {
        let mut disconnected = false;
        let received = self.load_rx.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(message) => Some(message),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                disconnected = true;
                None
            }
        });
        if disconnected {
            self.load_rx = None;
            self.load_label = None;
            self.set_status(
                StatusKind::Error,
                "The audio decoder stopped unexpectedly. Try loading the stem again.",
            );
            self.start_next_load();
            return;
        }
        let Some(LoadMessage(kind, result)) = received else {
            return;
        };
        self.load_rx = None;
        self.load_label = None;
        match result {
            Ok(track) => {
                let track = Arc::new(track);
                match kind {
                    Stem::Instrumental => {
                        self.project.instrumental = Some(track.path.clone());
                        self.wave_tex = Some(make_wave_texture(ctx, &track));
                        self.instrument = Some(track.clone());
                        self.install_engine(0, false);
                        self.tap_index = self
                            .project
                            .first_untimed()
                            .unwrap_or(self.project.token_count());
                        let duration = format_time(track.duration_ms);
                        let name = track.label();
                        self.set_status(
                            StatusKind::Good,
                            format!("Instrumental loaded: {name} ({duration})."),
                        );
                    }
                    Stem::Vocal => {
                        self.project.vocal = Some(track.path.clone());
                        if self.instrument.is_none() {
                            self.wave_tex = Some(make_wave_texture(ctx, &track));
                        }
                        self.vocal = Some(track.clone());
                        let (position, playing) = self
                            .engine
                            .as_ref()
                            .map(|e| (e.position_ms(), e.is_playing()))
                            .unwrap_or((self.manual_position, false));
                        if self.instrument.is_some() {
                            self.install_engine(position, playing);
                        }
                        self.set_status(
                            StatusKind::Good,
                            format!(
                                "Guide vocal loaded: {}. Monitor only — renders use the instrumental.",
                                track.label()
                            ),
                        );
                    }
                }
            }
            Err(error) => {
                self.set_status(StatusKind::Error, error.clone());
                self.error_dialog = Some(error);
            }
        }
        self.start_next_load();
    }

    fn install_engine(&mut self, start_ms: u64, play: bool) {
        self.engine = None;
        let Some(instrumental) = self.instrument.clone() else {
            return;
        };
        match AudioEngine::new(
            instrumental,
            self.vocal.clone(),
            start_ms,
            self.project.preroll_ms(),
        ) {
            Ok(engine) => {
                engine.set_gains(self.inst_gain, self.vocal_gain);
                engine.set_mutes(self.mute_inst, self.mute_vocal);
                if play {
                    engine.play();
                }
                self.engine = Some(engine);
                self.engine_error = None;
            }
            Err(error) => {
                self.engine_error = Some(error.clone());
                self.set_status(StatusKind::Error, format!("Audio unavailable: {error}"));
            }
        }
    }

    fn load_stem_dialog(&mut self, kind: Stem) {
        let dialog = rfd::FileDialog::new()
            .set_title(match kind {
                Stem::Instrumental => "Choose the instrumental stem",
                Stem::Vocal => "Choose the guide vocal stem",
            })
            .add_filter("Audio stems", &["flac", "wav", "mp3", "ogg", "m4a"])
            .set_directory(self.start_dir());
        if let Some(path) = dialog.pick_file() {
            self.load_stem(kind, path);
        }
    }

    fn load_stem(&mut self, kind: Stem, path: PathBuf) {
        match kind {
            Stem::Instrumental => {
                self.project.instrumental = Some(path.clone());
                self.instrument = None;
                self.engine = None;
                self.wave_tex = None;
            }
            Stem::Vocal => {
                self.project.vocal = Some(path.clone());
                self.vocal = None;
            }
        }
        self.set_status(StatusKind::Info, format!("Loading {}…", path.display()));
        self.enqueue_stem(kind, path);
    }

    fn pick_project_file(&self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Open KaraokeKit project or LRC")
            .add_filter("KaraokeKit project", &["json"])
            .add_filter("LRC lyrics", &["lrc"])
            .set_directory(self.start_dir())
            .pick_file()
    }

    fn open_project_dialog(&mut self) {
        if let Some(path) = self.pick_project_file() {
            self.request_open_path(path);
        }
    }

    /// Open picker that skips the dirty check (already confirmed by the user).
    fn choose_and_open_project(&mut self) {
        if let Some(path) = self.pick_project_file() {
            self.open_path(path);
        }
    }

    pub fn open_path(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "lrc" {
            self.import_lrc(&path);
        } else {
            self.open_project(&path);
        }
    }

    fn open_project(&mut self, path: &Path) {
        match Project::load_json(path) {
            Ok(project) => {
                self.project = project;
                self.project_path = Some(path.to_owned());
                self.instrument = None;
                self.vocal = None;
                self.engine = None;
                self.wave_tex = None;
                self.cover_tex = None;
                self.cover_key = None;
                self.bg_thumb = None;
                self.bg_thumb_key = None;
                self.selected = None;
                self.tap_index = self
                    .project
                    .first_untimed()
                    .unwrap_or(self.project.token_count());
                self.scroll_to_selected = true;
                self.sync_lyrics_draft();
                self.export_path = Some(self.start_dir().join(format!(
                    "{}.{}",
                    util::sanitize_filename(&self.project.title),
                    self.project.export.container.extension()
                )));
                self.load_queue.clear();
                if let Some(instrumental) = self.project.instrumental.clone() {
                    self.enqueue_stem(Stem::Instrumental, instrumental);
                }
                if let Some(vocal) = self.project.vocal.clone() {
                    self.enqueue_stem(Stem::Vocal, vocal);
                }
                self.set_status(
                    StatusKind::Good,
                    format!(
                        "Opened {} ({} tokens).",
                        path.display(),
                        self.project.token_count()
                    ),
                );
                self.mark_saved();
            }
            Err(error) => {
                self.set_status(StatusKind::Error, error.clone());
                self.error_dialog = Some(error);
            }
        }
    }

    fn import_lrc(&mut self, path: &Path) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let document = lrc::parse_lrc(&text);
                if document.lines.is_empty() {
                    self.set_status(
                        StatusKind::Error,
                        "No timestamped lyric lines found in LRC.",
                    );
                    return;
                }
                lrc::apply_lrc(&mut self.project, &document);
                self.selected = None;
                self.tap_index = self
                    .project
                    .first_untimed()
                    .unwrap_or(self.project.token_count());
                self.sync_lyrics_draft();
                self.set_status(
                    StatusKind::Good,
                    format!(
                        "Imported {} LRC lines; timings are ready to refine.",
                        self.project.lines.len()
                    ),
                );
            }
            Err(error) => {
                let message = format!("{}: {error}", path.display());
                self.set_status(StatusKind::Error, message.clone());
                self.error_dialog = Some(message);
            }
        }
    }

    fn save_project(&mut self, save_as: bool) -> bool {
        let path = if save_as || self.project_path.is_none() {
            let dialog = rfd::FileDialog::new()
                .set_title("Save KaraokeKit project")
                .set_file_name(format!(
                    "{}.json",
                    util::sanitize_filename(&self.project.title)
                ))
                .add_filter("KaraokeKit project", &["json"])
                .set_directory(self.start_dir());
            match dialog.save_file() {
                Some(path) => path,
                None => return false,
            }
        } else {
            self.project_path.clone().unwrap()
        };
        let path = if path.extension().and_then(|e| e.to_str()) != Some("json") {
            path.with_extension("json")
        } else {
            path
        };
        match self.project.save_json(&path) {
            Ok(()) => {
                self.project_path = Some(path.clone());
                if self.export_path.is_none() {
                    self.export_path =
                        Some(path.with_extension(self.project.export.container.extension()));
                }
                self.set_status(StatusKind::Good, format!("Saved {}", path.display()));
                self.mark_saved();
                true
            }
            Err(error) => {
                self.set_status(StatusKind::Error, error.clone());
                self.error_dialog = Some(error);
                false
            }
        }
    }

    fn export_lrc_dialog(&mut self) {
        let dialog = rfd::FileDialog::new()
            .set_title("Export LRC lyrics")
            .set_file_name(format!(
                "{}.lrc",
                util::sanitize_filename(&self.project.title)
            ))
            .add_filter("LRC lyrics", &["lrc"])
            .set_directory(self.start_dir());
        if let Some(path) = dialog.save_file() {
            let path = if path.extension().and_then(|e| e.to_str()) != Some("lrc") {
                path.with_extension("lrc")
            } else {
                path
            };
            let text = lrc::export_lrc(&self.project);
            match util::write_file(&path, text.as_bytes()) {
                Ok(()) => self.set_status(StatusKind::Good, format!("Exported {}", path.display())),
                Err(error) => {
                    self.set_status(StatusKind::Error, error.clone());
                    self.error_dialog = Some(error);
                }
            }
        }
    }

    fn export_ass_dialog(&mut self) {
        let dialog = rfd::FileDialog::new()
            .set_title("Export ASS karaoke subtitles")
            .set_file_name(format!(
                "{}.ass",
                util::sanitize_filename(&self.project.title)
            ))
            .add_filter("Advanced SubStation Alpha", &["ass"])
            .set_directory(self.start_dir());
        if let Some(path) = dialog.save_file() {
            let path = if path.extension().and_then(|e| e.to_str()) != Some("ass") {
                path.with_extension("ass")
            } else {
                path
            };
            let ass = ass::build_ass(&self.project);
            match util::write_file(&path, ass.as_bytes()) {
                Ok(()) => self.set_status(StatusKind::Good, format!("Exported {}", path.display())),
                Err(error) => {
                    self.set_status(StatusKind::Error, error.clone());
                    self.error_dialog = Some(error);
                }
            }
        }
    }

    fn new_project(&mut self) {
        self.project = Project::default();
        self.project_path = None;
        self.lyrics_draft.clear();
        self.instrument = None;
        self.vocal = None;
        self.engine = None;
        self.engine_error = None;
        self.load_queue.clear();
        self.load_rx = None;
        self.load_label = None;
        self.wave_tex = None;
        self.cover_tex = None;
        self.cover_key = None;
        self.bg_thumb = None;
        self.bg_thumb_key = None;
        self.selected = None;
        self.tap_index = 0;
        self.manual_position = 0;
        self.export_path = None;
        self.set_status(
            StatusKind::Info,
            "New project — load stems and paste lyrics.",
        );
        self.mark_saved();
    }

    fn choose_output(&mut self) -> Option<PathBuf> {
        let ext = self.project.export.container.extension();
        let suggested = self
            .export_path
            .clone()
            .unwrap_or_else(|| {
                self.start_dir().join(format!(
                    "{}.{}",
                    util::sanitize_filename(&self.project.title),
                    ext
                ))
            })
            .with_extension(ext);
        let dialog = rfd::FileDialog::new()
            .set_title("Choose the video output file")
            .set_file_name(
                suggested
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("karaoke.{ext}")),
            )
            .add_filter(
                if self.project.export.container == Container::Mp4 {
                    "MP4 video"
                } else {
                    "Matroska video"
                },
                &[ext],
            )
            .set_directory(
                suggested
                    .parent()
                    .map(Path::to_owned)
                    .unwrap_or_else(|| self.start_dir()),
            );
        let path = dialog.save_file()?;
        let path = if path.extension().and_then(|e| e.to_str()) != Some(ext) {
            path.with_extension(ext)
        } else {
            path
        };
        self.export_path = Some(path.clone());
        Some(path)
    }

    fn render_now(&mut self) {
        if self.render.is_some() {
            return;
        }
        if matches!(self.ffmpeg_status, FfmpegStatus::Missing(_)) {
            let message = match &self.ffmpeg_status {
                FfmpegStatus::Missing(error) => error.clone(),
                _ => String::new(),
            };
            self.error_dialog = Some(format!(
                "{message}\n\nInstall FFmpeg (for example `sudo pacman -S ffmpeg` or \
                 `winget install Gyan.FFmpeg`) and restart KaraokeKit."
            ));
            return;
        }
        let mut issues = self.project.validate_for_render();
        if self.instrument.is_none() {
            issues.push(
                "Load the instrumental stem first so KaraokeKit can read the song duration."
                    .to_owned(),
            );
        }
        if !issues.is_empty() {
            let message = format!("Fix these before rendering:\n\n• {}", issues.join("\n• "));
            self.set_status(StatusKind::Error, "Render blocked — see the checklist.");
            self.error_dialog = Some(message);
            return;
        }
        let duration = self.instrument.as_ref().map(|t| t.duration_ms).unwrap_or(0);
        let Some(mut output) = self.export_path.clone().or_else(|| self.choose_output()) else {
            return;
        };
        let ext = self.project.export.container.extension();
        if output.extension().and_then(|e| e.to_str()) != Some(ext) {
            output.set_extension(ext);
        }
        self.export_path = Some(output.clone());
        match export::prepare(&self.project, &output, duration) {
            Ok(plan) => {
                let handle = export::start(plan);
                self.render = Some(ActiveRender {
                    handle,
                    progress: 0.0,
                    speed: None,
                    cancelling: false,
                    started: Instant::now(),
                });
                self.last_render = None;
                self.set_status(StatusKind::Info, format!("Rendering {}…", output.display()));
            }
            Err(error) => {
                self.set_status(StatusKind::Error, error.clone());
                self.error_dialog = Some(error);
            }
        }
    }

    fn poll_render(&mut self) {
        let mut finished: Option<Result<PathBuf, (String, bool)>> = None;
        if let Some(active) = &mut self.render {
            loop {
                match active.handle.events.try_recv() {
                    Ok(RenderEvent::Progress { fraction, speed }) => {
                        active.progress = fraction.clamp(0.0, 1.0);
                        if speed.is_some() {
                            active.speed = speed;
                        }
                    }
                    Ok(RenderEvent::Done) => {
                        finished = Some(Ok(active.handle.output.clone()));
                        break;
                    }
                    Ok(RenderEvent::Failed(message)) => {
                        finished = Some(Err((message, false)));
                        break;
                    }
                    Ok(RenderEvent::Cancelled) => {
                        finished = Some(Err(("Render cancelled.".to_owned(), true)));
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = Some(Err((
                            "The renderer stopped unexpectedly.".to_owned(),
                            false,
                        )));
                        break;
                    }
                }
            }
        }
        match finished {
            Some(Ok(output)) => {
                self.render = None;
                self.set_status(
                    StatusKind::Good,
                    format!("Video finished: {}", output.display()),
                );
                self.last_render = Some((format!("Saved {}", output.display()), true));
            }
            Some(Err((message, cancelled))) => {
                self.render = None;
                if cancelled {
                    self.last_render = Some((message.clone(), false));
                    self.set_status(StatusKind::Warn, message);
                } else {
                    self.set_status(StatusKind::Error, "Render failed — see the log.");
                    self.error_dialog = Some(message.clone());
                    self.last_render = Some((message, false));
                }
            }
            None => {}
        }
    }

    fn poll_ffmpeg(&mut self) {
        let Some(rx) = &self.ffmpeg_rx else { return };
        match rx.try_recv() {
            Ok(Ok(version)) => {
                self.ffmpeg_status = FfmpegStatus::Ready(version);
                self.ffmpeg_rx = None;
            }
            Ok(Err(error)) => {
                self.ffmpeg_status = FfmpegStatus::Missing(error);
                self.ffmpeg_rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.ffmpeg_status = FfmpegStatus::Missing("FFmpeg check did not complete.".into());
                self.ffmpeg_rx = None;
            }
        }
    }

    fn handle_dropped(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "json" | "lrc" => self.request_open_path(path),
            "flac" | "wav" | "mp3" | "ogg" | "m4a" => {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                let looks_vocal = name.contains("vocal") || name.contains("guide");
                let kind = if looks_vocal && self.project.vocal.is_none() {
                    Stem::Vocal
                } else if self.project.instrumental.is_none() {
                    Stem::Instrumental
                } else {
                    Stem::Vocal
                };
                self.load_stem(kind, path);
            }
            "png" | "jpg" | "jpeg" | "webp" => {
                self.project.style.background = Background::Cover {
                    path: Some(path),
                    blur: 12.0,
                    dim: 0.25,
                };
                self.tab = SideTab::Style;
                self.set_status(StatusKind::Info, "Cover image set as background.");
            }
            "mp4" | "mov" | "mkv" | "webm" | "m4v" => {
                self.project.style.background = Background::Video {
                    path: Some(path),
                    dim: 0.3,
                };
                self.tab = SideTab::Style;
                self.set_status(
                    StatusKind::Info,
                    "Background video set. Preview shows a still frame; export loops the video.",
                );
            }
            _ => self.set_status(
                StatusKind::Warn,
                format!("Unsupported dropped file: {}", path.display()),
            ),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.text_edit_focused()
            || self.lyrics_window
            || self.help_window
            || self.error_dialog.is_some()
            || self.pending_action.is_some();
        if typing {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
            let save_as = ctx.input(|i| i.modifiers.shift);
            self.save_project(save_as);
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) {
            self.request_open_dialog();
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
            self.tap();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            self.toggle_play();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)) {
            let shift = ctx.input(|i| i.modifiers.shift);
            self.undo_tap(!shift);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Comma)) {
            self.nudge_selected(-25);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Period)) {
            self.nudge_selected(25);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)) {
            let step = if ctx.input(|i| i.modifiers.shift) {
                1_000
            } else {
                5_000
            };
            self.seek_relative(-(step as i64));
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)) {
            let step = if ctx.input(|i| i.modifiers.shift) {
                1_000
            } else {
                5_000
            };
            self.seek_relative(step as i64);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Home)) {
            self.seek(0);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F1)) {
            self.help_window = true;
        }
    }

    fn set_background_mode(&mut self, mode: usize) {
        let cover = match &self.project.style.background {
            Background::Cover { path, blur, dim } => Some((path.clone(), *blur, *dim)),
            _ => None,
        };
        let video = match &self.project.style.background {
            Background::Video { path, dim } => Some((path.clone(), *dim)),
            _ => None,
        };
        let gradient = match &self.project.style.background {
            Background::Gradient { top, bottom } => Some((*top, *bottom)),
            _ => None,
        };
        let cover_path = cover.as_ref().and_then(|c| c.0.clone());
        let cover_blur = cover.as_ref().map(|c| c.1).unwrap_or(12.0);
        let cover_dim = cover.as_ref().map(|c| c.2).unwrap_or(0.25);
        let video_path = video.as_ref().and_then(|v| v.0.clone());
        let video_dim = video.as_ref().map(|v| v.1).unwrap_or(0.3);
        self.project.style.background = match mode {
            0 => Background::Gradient {
                top: gradient.map(|g| g.0).unwrap_or([11, 16, 32]),
                bottom: gradient.map(|g| g.1).unwrap_or([27, 15, 43]),
            },
            1 => Background::Cover {
                path: cover_path,
                blur: cover_blur,
                dim: cover_dim,
            },
            _ => Background::Video {
                path: video_path,
                dim: video_dim,
            },
        };
        self.cover_tex = None;
        self.cover_key = None;
        self.bg_thumb = None;
        self.bg_thumb_key = None;
    }

    fn choose_cover(&mut self) {
        let dialog = rfd::FileDialog::new()
            .set_title("Choose a cover image")
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .set_directory(self.start_dir());
        if let Some(path) = dialog.pick_file() {
            self.project.style.background = Background::Cover {
                path: Some(path),
                blur: 12.0,
                dim: 0.25,
            };
            self.cover_tex = None;
            self.cover_key = None;
        }
    }

    fn choose_background_video(&mut self) {
        let dialog = rfd::FileDialog::new()
            .set_title("Choose a background video")
            .add_filter("Videos", &["mp4", "mov", "mkv", "webm", "m4v"])
            .set_directory(self.start_dir());
        if let Some(path) = dialog.pick_file() {
            self.project.style.background = Background::Video {
                path: Some(path),
                dim: 0.3,
            };
            self.bg_thumb = None;
            self.bg_thumb_key = None;
        }
    }

    fn ensure_background_textures(&mut self, ctx: &egui::Context) {
        let background = self.project.style.background.clone();
        match background {
            Background::Cover {
                path: Some(path), ..
            } => {
                let key = format!("cover:{}", path.display());
                if self.cover_key.as_deref() != Some(&key) {
                    self.cover_key = Some(key.clone());
                    match load_image_texture(ctx, &path, &key) {
                        Ok(texture) => self.cover_tex = Some(texture),
                        Err(error) => {
                            self.cover_tex = None;
                            self.set_status(StatusKind::Error, error);
                        }
                    }
                }
                if self.bg_thumb.is_some() {
                    self.bg_thumb = None;
                    self.bg_thumb_key = None;
                }
            }
            Background::Cover { path: None, .. } => {
                self.cover_tex = None;
                self.cover_key = None;
            }
            Background::Video {
                path: Some(path), ..
            } => {
                let key = format!("video:{}", path.display());
                if self.bg_thumb_key.as_deref() != Some(&key) {
                    self.bg_thumb_key = Some(key.clone());
                    match extract_video_thumb(ctx, &path, &key) {
                        Ok(texture) => self.bg_thumb = Some(texture),
                        Err(error) => {
                            self.bg_thumb = None;
                            self.set_status(StatusKind::Warn, error);
                        }
                    }
                }
                if self.cover_tex.is_some() {
                    self.cover_tex = None;
                    self.cover_key = None;
                }
            }
            Background::Video { path: None, .. } => {
                self.bg_thumb = None;
                self.bg_thumb_key = None;
            }
            Background::Gradient { .. } => {
                self.cover_tex = None;
                self.cover_key = None;
                self.bg_thumb = None;
                self.bg_thumb_key = None;
            }
        }
    }

    // ----- ui ------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("KaraokeKit")
                    .size(23.0)
                    .strong()
                    .color(theme::ACCENT),
            )
            .on_hover_text("Ready to make your next karaoke track 20% cooler.");
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .size(10.0)
                    .color(theme::MUTED),
            )
            .on_hover_text("Ready to make your next karaoke track 20% cooler.");
            ui.label(
                RichText::new("tap-to-sync karaoke video builder")
                    .size(11.0)
                    .color(theme::MUTED),
            );
            ui.add_space(18.0);
            if ui.button("New").clicked() {
                self.request_new();
            }
            if ui.button("Open").clicked() {
                self.request_open_dialog();
            }
            if ui.button("Save").clicked() {
                self.save_project(false);
            }
            if ui.button("Save LRC").clicked() {
                self.export_lrc_dialog();
            }
            if ui.button("Export .ass").clicked() {
                self.export_ass_dialog();
            }
            let render_label = if self.render.is_some() {
                "Rendering…"
            } else {
                "Render video"
            };
            if ui
                .add_enabled(self.render.is_none(), Button::new(render_label))
                .clicked()
            {
                self.render_now();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (dot, text, color) = match &self.ffmpeg_status {
                    FfmpegStatus::Checking => ("…", "Checking FFmpeg", theme::MUTED),
                    FfmpegStatus::Ready(_) => ("●", "FFmpeg ready", theme::OK),
                    FfmpegStatus::Missing(_) => ("●", "FFmpeg missing", theme::DANGER),
                };
                ui.label(RichText::new(text).size(11.0).color(color));
                ui.label(RichText::new(dot).size(10.0).color(color));
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let color = match self.status_kind {
                StatusKind::Info => theme::MUTED,
                StatusKind::Good => theme::OK,
                StatusKind::Warn => theme::GOLD,
                StatusKind::Error => theme::DANGER,
            };
            ui.add(Label::new(RichText::new(&self.status).size(12.0).color(color)).truncate());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(render) = &self.render {
                    let elapsed = render.started.elapsed().as_secs();
                    let label = if render.cancelling {
                        "Cancelling…".to_owned()
                    } else {
                        format!(
                            "Rendering {:.0}%  ·  {}  ·  {elapsed}s",
                            render.progress * 100.0,
                            render.speed.clone().unwrap_or_else(|| "—".into())
                        )
                    };
                    ui.add(
                        ProgressBar::new(render.progress)
                            .desired_width(200.0)
                            .text(label),
                    );
                } else {
                    ui.label(
                        RichText::new(format!(
                            "{} / {} tokens timed  ·  {} lines",
                            self.project.timed_count(),
                            self.project.token_count(),
                            self.project.lines.len()
                        ))
                        .size(11.0)
                        .color(theme::MUTED),
                    );
                }
            });
        });
    }

    fn left_panel(&mut self, ui: &mut Ui) {
        ScrollArea::vertical()
            .id_salt("left-rail")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.song_card(ui);
                ui.add_space(6.0);
                self.audio_card(ui);
                ui.add_space(6.0);
                self.transport_card(ui);
                ui.add_space(6.0);
                self.tap_card(ui);
                ui.add_space(6.0);
                self.lyrics_card(ui);
                ui.add_space(20.0);
            });
    }

    fn song_card(&mut self, ui: &mut Ui) {
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Song");
            ui.add(
                TextEdit::singleline(&mut self.project.title)
                    .desired_width(f32::INFINITY)
                    .hint_text("Song title"),
            );
            ui.add(
                TextEdit::singleline(&mut self.project.artist)
                    .desired_width(f32::INFINITY)
                    .hint_text("Artist"),
            );
            if let Some(path) = &self.project_path {
                ui.label(
                    RichText::new(
                        path.file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    )
                    .size(10.0)
                    .color(theme::MUTED),
                );
            }
        });
    }

    fn audio_card(&mut self, ui: &mut Ui) {
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Stems");

            let inst_label = self
                .instrument
                .as_ref()
                .map(|t| t.label())
                .unwrap_or_else(|| "No instrumental loaded".to_owned());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Instrumental").strong());
                    ui.label(RichText::new(inst_label).size(10.5).color(theme::MUTED));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Load…").clicked() {
                        self.load_stem_dialog(Stem::Instrumental);
                    }
                });
            });
            ui.add(
                Slider::new(&mut self.inst_gain, 0.0..=1.5)
                    .text("Level")
                    .show_value(false),
            );
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.mute_inst, "Mute instrumental");
                if let Some(track) = &self.instrument {
                    ui.label(
                        RichText::new(format_time(track.duration_ms))
                            .monospace()
                            .size(10.0)
                            .color(theme::MUTED),
                    );
                }
            });

            ui.separator();

            let vocal_label = self
                .vocal
                .as_ref()
                .map(|t| t.label())
                .unwrap_or_else(|| "Optional guide vocal".to_owned());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Guide vocal").strong());
                    ui.label(RichText::new(vocal_label).size(10.5).color(theme::MUTED));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Load…").clicked() {
                        self.load_stem_dialog(Stem::Vocal);
                    }
                });
            });
            ui.add(
                Slider::new(&mut self.vocal_gain, 0.0..=1.5)
                    .text("Monitor")
                    .show_value(false),
            );
            ui.checkbox(&mut self.mute_vocal, "Mute guide vocal");

            if let Some(label) = &self.load_label {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(label).size(11.0).color(theme::GOLD));
                });
            }
            if let Some(error) = &self.engine_error {
                ui.label(RichText::new(error).size(10.5).color(theme::DANGER));
            }
        });
    }

    fn transport_card(&mut self, ui: &mut Ui) {
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Transport");
            let playing = self
                .engine
                .as_ref()
                .map(|e| e.is_playing())
                .unwrap_or(false);
            let position = self.current_position();
            let duration = self.timeline_duration();
            ui.horizontal(|ui| {
                if ui.button("|<").clicked() {
                    self.seek(0);
                }
                if ui.button("-5s").clicked() {
                    self.seek_relative(-5_000);
                }
                let label = if playing { "Pause" } else { "Play" };
                let button = Button::new(RichText::new(label).strong())
                    .fill(theme::ACCENT_SOFT)
                    .min_size(Vec2::new(88.0, 30.0));
                if ui.add(button).clicked() {
                    self.toggle_play();
                }
                if ui.button("+5s").clicked() {
                    self.seek_relative(5_000);
                }
            });
            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    format_time(position),
                    format_time(duration)
                ))
                .monospace()
                .size(13.0)
                .color(theme::TEXT),
            );
            let mut scrub = position as f64 / 1000.0;
            let max = (duration as f64 / 1000.0).max(0.1);
            if ui
                .add(egui::Slider::new(&mut scrub, 0.0..=max).show_value(false))
                .changed()
            {
                self.seek((scrub * 1000.0) as u64);
            }
            if ui.button("Set selected token to playhead").clicked() {
                self.set_selected_to_playhead();
            }
        });
    }

    fn tap_card(&mut self, ui: &mut Ui) {
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Tap sync");
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.tap_mode, "Tap mode");
                ui.checkbox(&mut self.follow_tap, "Follow");
            });
            let next = self.next_untimed();
            match next {
                Some(index) => {
                    let token = self
                        .project
                        .token(index)
                        .map(|t| t.text.clone())
                        .unwrap_or_default();
                    let line = self
                        .project
                        .locate(index)
                        .map(|(l, _, _)| l + 1)
                        .unwrap_or(0);
                    ui.label(
                        RichText::new(format!("Next: “{token}”"))
                            .size(17.0)
                            .strong()
                            .color(theme::GOLD),
                    );
                    ui.label(
                        RichText::new(format!(
                            "Token {} of {}  ·  line {}",
                            index + 1,
                            self.project.token_count(),
                            line
                        ))
                        .size(10.5)
                        .color(theme::MUTED),
                    );
                }
                None if self.project.token_count() == 0 => {
                    ui.label(
                        RichText::new("Paste lyrics, then tokenize to begin.")
                            .size(13.0)
                            .color(theme::MUTED),
                    );
                }
                None => {
                    ui.label(
                        RichText::new("All tokens are timed.")
                            .size(15.0)
                            .strong()
                            .color(theme::OK),
                    );
                }
            }
            let total = self.project.token_count().max(1);
            let timed = self.project.timed_count();
            ui.add(
                ProgressBar::new(timed as f32 / total as f32)
                    .desired_width(f32::INFINITY)
                    .text(format!("{timed} / {}", self.project.token_count())),
            );
            ui.horizontal(|ui| {
                if ui
                    .add(
                        Button::new(RichText::new("Start tapping").strong())
                            .fill(theme::ACCENT_SOFT),
                    )
                    .clicked()
                {
                    self.start_tapping();
                }
                if ui.button("Undo tap").clicked() {
                    self.undo_tap(true);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Offset");
                ui.add(
                    egui::DragValue::new(&mut self.project.tap_offset_ms)
                        .speed(5.0)
                        .suffix(" ms"),
                )
                .on_hover_text(
                    "Shifts the whole song. Negative moves lyrics earlier — useful for \
                     Bluetooth/output latency.",
                );
            });
            ui.horizontal(|ui| {
                ui.label("Last token tail");
                ui.add(
                    egui::DragValue::new(&mut self.project.tail_ms)
                        .range(100..=5_000)
                        .speed(10.0)
                        .suffix(" ms"),
                );
            });
            ui.label(
                RichText::new("Space tap  ·  Backspace undo  ·  Enter play/pause")
                    .size(10.0)
                    .color(theme::MUTED),
            );
        });
    }

    fn lyrics_card(&mut self, ui: &mut Ui) {
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Lyrics");
            ui.label(
                RichText::new(format!(
                    "{} lines  ·  {} syllables",
                    self.project.lines.len(),
                    self.project.token_count()
                ))
                .size(11.0)
                .color(theme::MUTED),
            );
            ui.horizontal(|ui| {
                if ui.button("Edit lyrics").clicked() {
                    self.sync_lyrics_draft();
                    self.lyrics_window = true;
                    self.focus_lyrics_editor = true;
                }
                if ui.button("Import LRC").clicked() {
                    let dialog = rfd::FileDialog::new()
                        .set_title("Import LRC")
                        .add_filter("LRC lyrics", &["lrc"])
                        .set_directory(self.start_dir());
                    if let Some(path) = dialog.pick_file() {
                        self.import_lrc(&path);
                    }
                }
            });
            if let Some(line) = self.project.lines.first() {
                let text = line.text();
                let short: String = text.chars().take(42).collect();
                ui.label(
                    RichText::new(short)
                        .size(10.5)
                        .italics()
                        .color(theme::MUTED),
                );
            }
        });
    }

    fn right_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.tab == SideTab::Style, "Style")
                .clicked()
            {
                self.tab = SideTab::Style;
            }
            if ui
                .selectable_label(self.tab == SideTab::Export, "Export")
                .clicked()
            {
                self.tab = SideTab::Export;
            }
        });
        ui.separator();
        ScrollArea::vertical()
            .id_salt("right-rail")
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                SideTab::Style => self.style_ui(ui),
                SideTab::Export => self.export_ui(ui),
            });
    }

    fn aspect_selector(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Aspect");
            ui.selectable_value(
                &mut self.project.export.aspect,
                AspectRatio::Widescreen,
                "16:9",
            );
            ui.selectable_value(&mut self.project.export.aspect, AspectRatio::Classic, "4:3");
        });
        let (width, height) = self.project.export.aspect.dimensions();
        ui.label(
            RichText::new(format!(
                "{width}×{height} · {} text width",
                self.project.export.aspect.wrap_width()
            ))
            .size(10.0)
            .color(theme::MUTED),
        );
    }

    fn preroll_controls(&mut self, ui: &mut Ui) {
        ui.label("Title-card pre-roll");
        let mut value = self.project.style.intro_preroll_secs;
        if ui
            .add(
                Slider::new(&mut value, 0.0..=6.0)
                    .step_by(0.5)
                    .suffix(" s")
                    .fixed_decimals(1),
            )
            .changed()
        {
            self.project.style.intro_preroll_secs = (value * 2.0).round() / 2.0;
        }
        let has_lyrics = self.project.token_count() > 0;
        if ui
            .add_enabled(has_lyrics, Button::new("Auto (3.5s)"))
            .on_hover_text(
                "Use a 3.5 s title-card pre-roll when the first lyric starts \
                 before 3.5 s; otherwise leave the pre-roll off.",
            )
            .clicked()
        {
            self.project.style.intro_preroll_secs = self.project.auto_preroll_secs();
            let value = self.project.style.intro_preroll_secs;
            self.set_status(
                StatusKind::Info,
                format!("Title-card pre-roll set to {value:.1} s."),
            );
        }
        ui.label(
            RichText::new("Adds title/artist + countdown; lyrics shift forward.")
                .size(9.5)
                .color(theme::MUTED),
        );
    }

    fn style_ui(&mut self, ui: &mut Ui) {
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Canvas");
            self.aspect_selector(ui);
            ui.checkbox(
                &mut self.project.style.lead_in_countdown,
                "Lead-in countdown",
            )
            .on_hover_text(
                "Show a 4-dot cue before the next line when it follows a gap of \
                 four seconds or more.",
            );
            self.preroll_controls(ui);
        });
        ui.add_space(6.0);
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Typography");
            ui.label("Font family (libass/system font)");
            ui.add(
                TextEdit::singleline(&mut self.project.style.font_family)
                    .desired_width(f32::INFINITY),
            );
            ui.label("Size (1080p pixels)");
            ui.add(
                Slider::new(&mut self.project.style.font_size, 24.0..=180.0)
                    .suffix(" px")
                    .fixed_decimals(0),
            );
            ui.horizontal(|ui| {
                ui.label("Base");
                ui.color_edit_button_srgb(&mut self.project.style.base_color);
                ui.label("Sweep");
                ui.color_edit_button_srgb(&mut self.project.style.highlight_color);
            });
            ui.label("Outline");
            ui.add(
                Slider::new(&mut self.project.style.outline_width, 0.0..=16.0).fixed_decimals(1),
            );
            ui.label("Shadow");
            ui.add(Slider::new(&mut self.project.style.shadow_width, 0.0..=16.0).fixed_decimals(1));
            ui.label("Bottom margin");
            ui.add(
                Slider::new(&mut self.project.style.margin_v, 0.0..=360.0)
                    .suffix(" px")
                    .fixed_decimals(0),
            );
        });

        ui.add_space(6.0);
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Background");
            let current = match self.project.style.background {
                Background::Gradient { .. } => 0,
                Background::Cover { .. } => 1,
                Background::Video { .. } => 2,
            };
            let mut selected = current;
            ComboBox::from_id_salt("background-mode")
                .selected_text(self.project.style.background.label())
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut selected, 0, "Solid dark gradient");
                    ui.selectable_value(&mut selected, 1, "Cover image");
                    ui.selectable_value(&mut selected, 2, "Background video");
                });
            if selected != current {
                self.set_background_mode(selected);
            }

            let mut choose_cover = false;
            let mut choose_video = false;
            match &mut self.project.style.background {
                Background::Gradient { top, bottom } => {
                    ui.horizontal(|ui| {
                        ui.label("Top");
                        ui.color_edit_button_srgb(top);
                        ui.label("Bottom");
                        ui.color_edit_button_srgb(bottom);
                    });
                }
                Background::Cover { path, blur, dim } => {
                    let name = path
                        .as_ref()
                        .map(|p| {
                            p.file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        })
                        .unwrap_or_else(|| "No image selected".to_owned());
                    ui.label(RichText::new(name).size(10.5).color(theme::MUTED));
                    if ui.button("Choose image…").clicked() {
                        choose_cover = true;
                    }
                    ui.add(Slider::new(blur, 0.0..=50.0).text("Blur").fixed_decimals(0));
                    ui.add(Slider::new(dim, 0.0..=0.9).text("Dim").fixed_decimals(2));
                }
                Background::Video { path, dim } => {
                    let name = path
                        .as_ref()
                        .map(|p| {
                            p.file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        })
                        .unwrap_or_else(|| "No video selected".to_owned());
                    ui.label(RichText::new(name).size(10.5).color(theme::MUTED));
                    if ui.button("Choose video…").clicked() {
                        choose_video = true;
                    }
                    ui.add(Slider::new(dim, 0.0..=0.9).text("Dim").fixed_decimals(2));
                    ui.label(
                        RichText::new("Exports loop the full video; preview shows a still.")
                            .size(10.0)
                            .color(theme::MUTED),
                    );
                }
            }
            if choose_cover {
                self.choose_cover();
            }
            if choose_video {
                self.choose_background_video();
            }
        });

        ui.add_space(6.0);
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Fonts & files");
            ui.label(
                RichText::new(
                    "DejaVu Sans is bundled and embedded, so burned-in lyrics match \
                     the preview on Windows without extra installs.",
                )
                .size(10.5)
                .color(theme::MUTED),
            );
        });
    }

    fn export_ui(&mut self, ui: &mut Ui) {
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Canvas");
            self.aspect_selector(ui);
            self.preroll_controls(ui);
            ui.separator();
            theme::section_label(ui, "Container");
            ui.radio_value(
                &mut self.project.export.container,
                Container::Mp4,
                Container::Mp4.label(),
            );
            ui.radio_value(
                &mut self.project.export.container,
                Container::Mkv,
                Container::Mkv.label(),
            );
            ui.add_space(4.0);
            theme::section_label(ui, "Frame rate");
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.project.export.fps, 30, "30 fps");
                ui.radio_value(&mut self.project.export.fps, 60, "60 fps");
            });
            if self.project.export.container == Container::Mkv {
                ui.add_space(4.0);
                theme::section_label(ui, "Audio codec");
                ui.radio_value(
                    &mut self.project.export.audio_codec,
                    AudioCodec::Aac,
                    AudioCodec::Aac.label(),
                );
                ui.radio_value(
                    &mut self.project.export.audio_codec,
                    AudioCodec::Flac,
                    AudioCodec::Flac.label(),
                );
            } else {
                ui.label(
                    RichText::new("MP4 always uses AAC 320k.")
                        .size(10.0)
                        .color(theme::MUTED),
                );
            }
            ui.add_space(4.0);
            theme::section_label(ui, "Quality");
            ui.add(
                Slider::new(&mut self.project.export.crf, 14..=28)
                    .text("CRF")
                    .fixed_decimals(0),
            );
            let presets = ["veryfast", "medium", "slow"];
            let selected = self.project.export.preset.clone();
            ComboBox::from_id_salt("x264-preset")
                .selected_text(selected.clone())
                .show_ui(ui, |ui| {
                    for preset in presets {
                        ui.selectable_value(
                            &mut self.project.export.preset,
                            preset.to_owned(),
                            preset,
                        );
                    }
                });
            if !presets.contains(&self.project.export.preset.as_str()) {
                self.project.export.preset = "medium".to_owned();
            }
        });

        ui.add_space(6.0);
        let mut cancel_requested = false;
        theme::card().show(ui, |ui| {
            theme::section_label(ui, "Output");
            let path_label = self
                .export_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Choose an output file…".to_owned());
            ui.add(Label::new(RichText::new(path_label).size(10.5).color(theme::MUTED)).truncate());
            if ui.button("Browse…").clicked() {
                self.choose_output();
            }
            ui.add_space(4.0);
            if let Some(render) = &self.render {
                ui.add(
                    ProgressBar::new(render.progress)
                        .desired_width(f32::INFINITY)
                        .show_percentage(),
                );
                ui.label(
                    RichText::new(format!(
                        "{}  ·  {}",
                        render.speed.clone().unwrap_or_else(|| "estimating…".into()),
                        render.handle.output.display()
                    ))
                    .size(10.0)
                    .color(theme::MUTED),
                );
                if render.cancelling {
                    ui.label(RichText::new("Cancelling…").color(theme::GOLD));
                } else if ui
                    .add(Button::new("Cancel render").fill(Color32::from_rgb(92, 40, 48)))
                    .clicked()
                {
                    cancel_requested = true;
                }
            } else {
                if ui
                    .add(
                        Button::new(RichText::new("Render video").strong())
                            .fill(theme::ACCENT)
                            .min_size(Vec2::new(ui.available_width(), 34.0)),
                    )
                    .clicked()
                {
                    self.render_now();
                }
            }
            if let Some((message, ok)) = &self.last_render {
                ui.add(
                    Label::new(RichText::new(message).size(10.5).color(if *ok {
                        theme::OK
                    } else {
                        theme::DANGER
                    }))
                    .truncate(),
                );
            }
            ui.separator();
            match &self.ffmpeg_status {
                FfmpegStatus::Checking => {
                    ui.label(RichText::new("Checking FFmpeg…").size(10.0));
                }
                FfmpegStatus::Ready(version) => {
                    ui.add(
                        Label::new(
                            RichText::new(version)
                                .size(9.5)
                                .color(theme::OK)
                                .monospace(),
                        )
                        .truncate(),
                    );
                }
                FfmpegStatus::Missing(error) => {
                    ui.add(
                        Label::new(RichText::new(error).size(10.0).color(theme::DANGER)).truncate(),
                    );
                }
            }
        });
        if cancel_requested {
            if let Some(render) = &mut self.render {
                render.cancelling = true;
                render.handle.request_cancel();
            }
        }
        if let Some(path) = &mut self.export_path {
            path.set_extension(self.project.export.container.extension());
        }
    }

    fn central_panel(&mut self, ui: &mut Ui) {
        let position = self.current_position();
        let duration = self.timeline_duration();
        let aspect = self.project.export.aspect;
        let (canvas_width, canvas_height) = aspect.dimensions();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Preview")
                    .size(11.0)
                    .strong()
                    .color(theme::MUTED),
            );
            ui.label(
                RichText::new(format!(
                    "{}  ·  {canvas_width}×{canvas_height}  ·  {} fps",
                    self.project.style.background.label(),
                    self.project.export.fps
                ))
                .size(10.0)
                .color(theme::MUTED),
            );
        });
        // 4:3 wraps into more rows, so give the stage a little more vertical room;
        // lyrics are still stacked using their real rendered heights.
        let preview_fraction = if aspect == AspectRatio::Classic {
            0.68
        } else {
            0.58
        };
        let preview_h =
            (ui.available_width() / aspect.ratio()).min(ui.available_height() * preview_fraction);
        ui.scope(|ui| {
            ui.set_max_height(preview_h);
            let _ = preview::show(
                ui,
                &self.project,
                position,
                self.cover_tex.as_ref(),
                self.bg_thumb.as_ref(),
            );
        });

        ui.add_space(8.0);
        let selected_line = self
            .selected
            .and_then(|flat| self.project.locate(flat).map(|(line, _, _)| line));
        let output = timeline::show(
            ui,
            &self.project,
            self.wave_tex.as_ref(),
            position,
            duration,
            self.project.preroll_ms(),
            selected_line,
        );
        if let Some(ms) = output.seek_ms {
            self.seek(ms);
        }

        ui.add_space(6.0);
        let rows = build_rows(&self.project);
        ui.horizontal(|ui| {
            theme::section_label(ui, "Sync tokens");
            ui.label(
                RichText::new(format!(
                    "{} / {} timed",
                    self.project.timed_count(),
                    self.project.token_count()
                ))
                .size(10.0)
                .color(theme::MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new("click a token to select · -25/+25 nudge")
                        .size(9.5)
                        .color(theme::MUTED),
                );
            });
        });

        let mut nudge: Option<(usize, i64)> = None;
        let mut set_now: Option<usize> = None;
        let mut goto: Option<u64> = None;
        let mut selected = self.selected;
        let spans = self.project.spans();

        let mut area = ScrollArea::vertical()
            .id_salt("token-list")
            .auto_shrink([false, false]);
        if self.scroll_to_selected {
            if let Some(target) = self.selected {
                if let Some(row_index) = rows
                    .iter()
                    .position(|row| matches!(row, TokenRow::Token { flat, .. } if *flat == target))
                {
                    let viewport = ui.available_height().max(140.0);
                    let offset = (row_index as f32 * 27.0 - viewport * 0.35).max(0.0);
                    area = area.vertical_scroll_offset(offset);
                }
            }
            self.scroll_to_selected = false;
        }

        area.show_rows(ui, 27.0, rows.len(), |ui, range| {
            for index in range {
                match &rows[index] {
                    TokenRow::Header(line) => {
                        let text = self
                            .project
                            .lines
                            .get(*line)
                            .map(|l| format!("Line {:02}  ·  {}", line + 1, l.text()))
                            .unwrap_or_default();
                        ui.add_space(4.0);
                        ui.label(RichText::new(text).size(10.0).color(theme::MUTED).italics());
                    }
                    TokenRow::Token { flat, line, index } => {
                        let token = &self.project.lines[*line].tokens[*index];
                        let text = if token.word_start {
                            format!(" {}", token.text)
                        } else {
                            token.text.clone()
                        };
                        let span = spans.get(*flat).copied().flatten();
                        let is_selected = selected == Some(*flat);
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [28.0, 20.0],
                                Label::new(
                                    RichText::new(format!("{:>4}", flat + 1))
                                        .monospace()
                                        .size(10.0)
                                        .color(theme::MUTED),
                                ),
                            );
                            if ui
                                .add_sized(
                                    [150.0, 20.0],
                                    Button::new(RichText::new(text).size(13.0))
                                        .selected(is_selected),
                                )
                                .clicked()
                            {
                                selected = Some(*flat);
                            }
                            match span {
                                Some((start, end)) => {
                                    ui.add_sized(
                                        [82.0, 20.0],
                                        Label::new(
                                            RichText::new(format_time(start))
                                                .monospace()
                                                .size(11.0),
                                        ),
                                    );
                                    ui.add_sized(
                                        [82.0, 20.0],
                                        Label::new(
                                            RichText::new(format_time(end))
                                                .monospace()
                                                .size(11.0)
                                                .color(theme::MUTED),
                                        ),
                                    );
                                    let dur = end.saturating_sub(start);
                                    ui.add_sized(
                                        [58.0, 20.0],
                                        Label::new(
                                            RichText::new(format!("{dur} ms"))
                                                .monospace()
                                                .size(10.0)
                                                .color(theme::MUTED),
                                        ),
                                    );
                                }
                                None => {
                                    ui.add_sized(
                                        [230.0, 20.0],
                                        Label::new(
                                            RichText::new("not tapped yet")
                                                .size(11.0)
                                                .color(theme::GOLD),
                                        ),
                                    );
                                }
                            }
                            if ui.small_button("-25").clicked() {
                                nudge = Some((*flat, -25));
                            }
                            if ui.small_button("+25").clicked() {
                                nudge = Some((*flat, 25));
                            }
                            if ui
                                .small_button("@")
                                .on_hover_text("Set this token's start to the playhead")
                                .clicked()
                            {
                                set_now = Some(*flat);
                            }
                            if span.is_some() && ui.small_button("Go").clicked() {
                                goto = span.map(|(s, _)| s);
                            }
                        });
                    }
                }
            }
        });
        self.selected = selected;
        if let Some((index, delta)) = nudge {
            self.selected = Some(index);
            self.project.nudge_token(index, delta);
        }
        if let Some(index) = set_now {
            self.selected = Some(index);
            self.set_selected_to_playhead();
        }
        if let Some(ms) = goto {
            self.seek(ms);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let mut lyrics_open = self.lyrics_window;
        if lyrics_open {
            let mut apply = false;
            let mut close = false;
            egui::Window::new("Lyrics editor")
                .open(&mut lyrics_open)
                .default_size(Vec2::new(680.0, 520.0))
                .resizable(true)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new(
                            "One line per lyric line. Put / inside a word to split syllables \
                             (won/der/ful). Tokenize to reset timings.",
                        )
                        .size(11.0)
                        .color(theme::MUTED),
                    );
                    let lyrics_edit_id = ui.make_persistent_id("karaokekit-lyrics-editor");
                    ui.horizontal(|ui| {
                        if ui.button("Paste from clipboard").clicked() {
                            ui.ctx().memory_mut(|m| m.request_focus(lyrics_edit_id));
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                        }
                        ui.label(
                            RichText::new("Ctrl+V or right-click → Paste")
                                .size(10.0)
                                .color(theme::MUTED),
                        );
                    });
                    ScrollArea::vertical()
                        .id_salt("lyrics-editor")
                        .max_height(360.0)
                        .show(ui, |ui| {
                            let output = TextEdit::multiline(&mut self.lyrics_draft)
                                .id(lyrics_edit_id)
                                .desired_width(f32::INFINITY)
                                .desired_rows(18)
                                .font(egui::TextStyle::Monospace)
                                .show(ui);

                            if self.focus_lyrics_editor {
                                ui.ctx().memory_mut(|m| m.request_focus(lyrics_edit_id));
                                self.focus_lyrics_editor = false;
                            }

                            let mut paste = false;
                            let mut copy = false;
                            let mut select_all = false;
                            let has_selection = output
                                .cursor_range
                                .map(|range| !range.is_empty())
                                .unwrap_or(false);

                            output.response.response.context_menu(|ui| {
                                if ui.button("Paste").clicked() {
                                    paste = true;
                                    ui.close();
                                }
                                if ui.add_enabled(has_selection, Button::new("Copy")).clicked() {
                                    copy = true;
                                    ui.close();
                                }
                                if ui.button("Select all").clicked() {
                                    select_all = true;
                                    ui.close();
                                }
                            });

                            if paste {
                                ui.ctx().memory_mut(|m| m.request_focus(lyrics_edit_id));
                                ui.ctx()
                                    .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                            }
                            if copy {
                                if let Some(range) = output.cursor_range {
                                    let selected = range.slice_str(&self.lyrics_draft).to_owned();
                                    if !selected.is_empty() {
                                        ui.ctx().copy_text(selected);
                                    }
                                }
                            }
                            if select_all {
                                let mut state = output.state;
                                state.cursor.set_char_range(Some(
                                    egui::text::CCursorRange::select_all_str(&self.lyrics_draft),
                                ));
                                state.store(ui.ctx(), lyrics_edit_id);
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui
                            .add(Button::new(RichText::new("Tokenize & apply").strong()))
                            .clicked()
                        {
                            apply = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            if apply && self.apply_lyrics() {
                lyrics_open = false;
            }
            if close {
                lyrics_open = false;
            }
            if !lyrics_open {
                self.focus_lyrics_editor = false;
            }
            self.lyrics_window = lyrics_open;
        }

        let mut help_open = self.help_window;
        if help_open {
            egui::Window::new("Shortcuts & tap-sync help")
                .open(&mut help_open)
                .default_width(460.0)
                .show(ctx, |ui| {
                    let rows = [
                        (
                            "Space",
                            "Tap the next syllable (or play/pause when Tap mode is off)",
                        ),
                        ("Enter", "Play / pause"),
                        ("Backspace", "Undo last tap and seek back to re-tap"),
                        ("Shift+Backspace", "Undo last tap without seeking"),
                        ("Left / Right", "Seek 5 s (Shift: 1 s)"),
                        ("Home", "Seek to start"),
                        (", / .", "Nudge selected token by ±25 ms"),
                        ("Ctrl+S", "Save project JSON"),
                        ("Ctrl+Shift+S", "Save project as…"),
                        ("Ctrl+O", "Open project or LRC"),
                        ("F1", "This help"),
                    ];
                    for (keys, description) in rows {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [130.0, 18.0],
                                Label::new(RichText::new(keys).monospace().color(theme::GOLD)),
                            );
                            ui.label(RichText::new(description).size(12.0));
                        });
                    }
                    ui.separator();
                    ui.label(
                        RichText::new(
                            "Workflow: load the instrumental + optional vocal, paste lyrics, \
                             press Start tapping, then Space on every syllable.",
                        )
                        .size(11.0)
                        .color(theme::MUTED),
                    );
                });
            self.help_window = help_open;
        }

        if let Some(message) = self.error_dialog.clone() {
            let mut open = true;
            let mut close = false;
            egui::Window::new("KaraokeKit")
                .open(&mut open)
                .default_width(560.0)
                .resizable(true)
                .collapsible(false)
                .show(ctx, |ui| {
                    ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                        ui.label(RichText::new(message).size(12.0).color(theme::TEXT));
                    });
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            if !open || close {
                self.error_dialog = None;
            }
        }

        if let Some(action) = self.pending_action.clone() {
            let mut choice = None;
            egui::Window::new("Unsaved changes")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("This project has unsaved changes.")
                            .size(13.0)
                            .color(theme::TEXT),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save and continue").clicked() {
                            choice = Some(0u8);
                        }
                        if ui.button("Discard changes").clicked() {
                            choice = Some(1u8);
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(2u8);
                        }
                    });
                });
            match choice {
                Some(0) => {
                    if self.save_project(false) {
                        self.pending_action = None;
                        self.perform_pending(action, ctx);
                    }
                }
                Some(1) => {
                    self.pending_action = None;
                    self.perform_pending(action, ctx);
                }
                Some(2) => self.pending_action = None,
                _ => {}
            }
        }
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = if self.project.title.trim().is_empty() {
            "KaraokeKit".to_owned()
        } else {
            format!("KaraokeKit — {}", self.project.title.trim())
        };
        if title != self.window_title {
            self.window_title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }
}

impl eframe::App for KaraokeKitApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Intercept the window close button when there are unsaved edits. This
        // lives in `logic` so it also runs when eframe skips the UI pass (e.g.
        // while the window is hidden or occluded). Keep canceling repeated close
        // requests while the confirmation dialog is up.
        if !self.confirmed_close && ctx.input(|i| i.viewport().close_requested()) && self.is_dirty()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.pending_action.is_none() {
                self.pending_action = Some(PendingAction::Close);
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.handle_shortcuts(&ctx);
        self.poll_ffmpeg();
        self.poll_load(&ctx);
        self.poll_render();

        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            self.handle_dropped(file.path().to_owned());
        }

        egui::Panel::top("top-bar")
            .frame(
                egui::Frame::NONE
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .stroke(Stroke::new(1.0, theme::BORDER)),
            )
            .show(ui, |ui| self.top_bar(ui));

        egui::Panel::bottom("status-bar")
            .exact_size(34.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 7))
                    .stroke(Stroke::new(1.0, theme::BORDER)),
            )
            .show(ui, |ui| self.status_bar(ui));

        egui::Panel::left("left-rail")
            .resizable(false)
            .exact_size(344.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::symmetric(10, 10)),
            )
            .show(ui, |ui| self.left_panel(ui));

        egui::Panel::right("right-rail")
            .resizable(false)
            .exact_size(330.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::symmetric(10, 10)),
            )
            .show(ui, |ui| self.right_panel(ui));

        self.ensure_background_textures(&ctx);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::symmetric(12, 10)),
            )
            .show(ui, |ui| self.central_panel(ui));

        if let Some(engine) = &self.engine {
            engine.set_gains(self.inst_gain, self.vocal_gain);
            engine.set_mutes(self.mute_inst, self.mute_vocal);
        }

        // Applying a title-card pre-roll shifts the whole timeline; rebase the
        // engine without changing the audio-relative position.
        let rebase = self.engine.as_ref().and_then(|engine| {
            let old = engine.preroll_ms();
            let new = self.project.preroll_ms();
            if old == new {
                return None;
            }
            let timeline_position = engine.position_ms();
            let next_position = if timeline_position <= old {
                0
            } else {
                timeline_position.saturating_sub(old) + new
            };
            Some((next_position, engine.is_playing()))
        });
        if let Some((start, playing)) = rebase {
            self.install_engine(start, playing);
        }

        self.dialogs(&ctx);
        self.update_window_title(&ctx);

        let busy = self
            .engine
            .as_ref()
            .map(|e| e.is_playing())
            .unwrap_or(false)
            || self.load_rx.is_some()
            || self.render.is_some()
            || self.ffmpeg_rx.is_some();
        if busy {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let family = FontFamily::Name(Arc::from(FONT_FAMILY));
    fonts.font_data.insert(
        "karaokekit-dejavu".to_owned(),
        Arc::new(egui::FontData::from_static(crate::assets::FONT_TTF)),
    );
    fonts
        .families
        .insert(family.clone(), vec!["karaokekit-dejavu".to_owned()]);
    if let Some(fallback) = fonts.families.get_mut(&FontFamily::Proportional) {
        fallback.push("karaokekit-dejavu".to_owned());
    }
    ctx.set_fonts(fonts);
}

fn make_wave_texture(ctx: &egui::Context, track: &Track) -> egui::TextureHandle {
    let width = track.peaks.len().max(1);
    let height = 128usize;
    let mut pixels = vec![Color32::TRANSPARENT; width * height];
    let center = height as f32 / 2.0;
    for (x, peak) in track.peaks.iter().enumerate() {
        let top = (center - peak.max * (height as f32 * 0.46)).round() as isize;
        let bottom = (center - peak.min * (height as f32 * 0.46)).round() as isize;
        let (top, bottom) = (top.min(bottom), top.max(bottom));
        for y in top.max(0) as usize..=(bottom.max(0) as usize).min(height - 1) {
            let distance = ((y as f32 - center) / center).abs();
            let alpha = (215.0 - distance * 80.0).clamp(80.0, 215.0) as u8;
            let color = Color32::from_rgba_unmultiplied(126, 201, 255, alpha);
            pixels[y * width + x] = color;
        }
        // At least one pixel so silence still reads as a line.
        if top == bottom && bottom >= 0 && (bottom as usize) < height {
            pixels[bottom as usize * width + x] =
                Color32::from_rgba_unmultiplied(126, 201, 255, 150);
        }
    }
    let image = egui::ColorImage {
        size: [width, height],
        pixels,
        source_size: Vec2::new(width as f32, height as f32),
    };
    ctx.load_texture(
        format!("waveform-{}", track.path.display()),
        image,
        egui::TextureOptions::LINEAR,
    )
}

fn load_image_texture(
    ctx: &egui::Context,
    path: &Path,
    key: &str,
) -> Result<egui::TextureHandle, String> {
    let image =
        image::open(path).map_err(|e| format!("Could not load image {}: {e}", path.display()))?;
    let image = if image.width() > 1920 || image.height() > 1920 {
        image.thumbnail(1920, 1920)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Ok(ctx.load_texture(key, color_image, egui::TextureOptions::LINEAR))
}

fn extract_video_thumb(
    ctx: &egui::Context,
    path: &Path,
    key: &str,
) -> Result<egui::TextureHandle, String> {
    let dir = util::temp_dir("karaokekit-thumb")
        .map_err(|e| format!("Could not create thumbnail folder: {e}"))?;
    let output = dir.join("thumb.png");
    let mut command = std::process::Command::new(util::ffmpeg_path());
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-ss",
            "0.5",
            "-i",
        ])
        .arg(path)
        .args(["-frames:v", "1", "-vf", "scale=1280:-2"])
        .arg(&output);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let status = command
        .status()
        .map_err(|e| format!("Could not run FFmpeg for the video preview: {e}"))?;
    if !status.success() || !output.exists() {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("Could not extract a frame from the background video.".to_owned());
    }
    let bytes =
        std::fs::read(&output).map_err(|e| format!("Could not read the video thumbnail: {e}"))?;
    let _ = std::fs::remove_dir_all(&dir);
    let image = image::load_from_memory(&bytes)
        .map_err(|e| format!("Could not decode the video thumbnail: {e}"))?;
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(ctx.load_texture(
        key,
        egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
        egui::TextureOptions::LINEAR,
    ))
}

fn build_rows(project: &Project) -> Vec<TokenRow> {
    let mut rows = Vec::new();
    for (line_index, line) in project.lines.iter().enumerate() {
        if line.tokens.is_empty() {
            continue;
        }
        rows.push(TokenRow::Header(line_index));
        for index in 0..line.tokens.len() {
            rows.push(TokenRow::Token {
                flat: project.flat_index(line_index, index).unwrap_or_default(),
                line: line_index,
                index,
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LyricLine, Token};

    #[test]
    fn project_revision_tracks_serialized_changes() {
        let mut project = Project::default();
        let saved = KaraokeKitApp::project_revision(&project);

        project.style.lead_in_countdown = !project.style.lead_in_countdown;
        assert_ne!(saved, KaraokeKitApp::project_revision(&project));

        project.style.lead_in_countdown = !project.style.lead_in_countdown;
        assert_eq!(saved, KaraokeKitApp::project_revision(&project));

        project.lines = vec![LyricLine {
            tokens: vec![Token {
                text: "la".into(),
                word_start: true,
                start_ms: Some(1_000),
                end_ms: None,
            }],
            end_ms: None,
        }];
        assert_ne!(saved, KaraokeKitApp::project_revision(&project));
    }
}
