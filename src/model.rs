//! Project data model and timing rules.
//!
//! Times are stored as **raw** millisecond timestamps; `tap_offset_ms` is applied
//! everywhere user-visible so a single calibration slider can shift the whole song
//! after tapping. Token ends are derived from the next token's start (the last
//! token gets `tail_ms`), which makes re-tapping a boundary fix both sides.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const PROJECT_VERSION: u32 = 1;
pub const DEFAULT_TAIL_MS: u64 = 900;

pub type Rgb = [u8; 3];

fn default_font() -> String {
    "DejaVu Sans".to_owned()
}
fn default_size() -> f32 {
    72.0
}
fn default_base() -> Rgb {
    [0xFF, 0xFF, 0xFF]
}
fn default_highlight() -> Rgb {
    [0xFF, 0xD5, 0x4A]
}
fn default_outline() -> f32 {
    3.0
}
fn default_shadow() -> f32 {
    2.0
}
fn default_margin_v() -> f32 {
    90.0
}
fn default_dim() -> f32 {
    0.25
}
fn default_blur() -> f32 {
    12.0
}
fn default_true() -> bool {
    true
}

/// How early the upcoming line may appear on deck.
pub const ON_DECK_LEAD_MS: u64 = 6_000;

/// A gap at least this long gets a lead-in countdown cue.
pub const LEAD_IN_GAP_MS: u64 = 4_000;
/// Countdown window immediately before the next line.
pub const COUNTDOWN_WINDOW_MS: u64 = 3_200;
/// One dot step.
pub const COUNTDOWN_STEP_MS: u64 = 800;
/// Countdown dots are this fraction of the lyric font size.
pub const COUNTDOWN_DOT_SCALE: f32 = 0.5;

/// Title-card pre-roll limits and title/artist layout.
pub const MAX_PREROLL_SECS: f32 = 6.0;
pub const PREROLL_TITLE_SCALE: f32 = 1.15;
pub const PREROLL_ARTIST_SCALE: f32 = 0.65;
pub const PREROLL_TITLE_Y: f32 = 0.22;
pub const PREROLL_ARTIST_Y: f32 = 0.32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountdownCue {
    /// Project line index of the upcoming line.
    pub line: usize,
    /// Number of visible dots (4, 3, 2, 1).
    pub dots: usize,
    pub next_start: u64,
}

/// Shared dot string for the preview and `.ass` exporter.
pub fn countdown_text(dots: usize) -> String {
    vec!["●"; dots.clamp(0, 4)].join(" ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineInterval {
    pub index: usize,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub version: u32,
    pub title: String,
    pub artist: String,
    pub instrumental: Option<PathBuf>,
    pub vocal: Option<PathBuf>,
    pub lines: Vec<LyricLine>,
    pub style: Style,
    pub export: ExportSettings,
    /// Global shift added to every timestamp (negative = lyrics earlier).
    pub tap_offset_ms: i64,
    /// Duration given to the final token of the song.
    pub tail_ms: u64,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            version: PROJECT_VERSION,
            title: "Untitled Song".to_owned(),
            artist: String::new(),
            instrumental: None,
            vocal: None,
            lines: Vec::new(),
            style: Style::default(),
            export: ExportSettings::default(),
            tap_offset_ms: 0,
            tail_ms: DEFAULT_TAIL_MS,
        }
    }
}

impl Project {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn token_count(&self) -> usize {
        self.lines.iter().map(|l| l.tokens.len()).sum()
    }

    pub fn tokens(&self) -> impl Iterator<Item = &Token> {
        self.lines.iter().flat_map(|l| l.tokens.iter())
    }

    pub fn timed_count(&self) -> usize {
        self.tokens().filter(|t| t.start_ms.is_some()).count()
    }

    pub fn all_tokens_timed(&self) -> bool {
        self.token_count() > 0 && self.timed_count() == self.token_count()
    }

    pub fn first_untimed(&self) -> Option<usize> {
        self.tokens().position(|t| t.start_ms.is_none())
    }

    pub fn token(&self, flat: usize) -> Option<&Token> {
        self.tokens().nth(flat)
    }

    pub fn token_mut(&mut self, flat: usize) -> Option<&mut Token> {
        let mut remaining = flat;
        for line in &mut self.lines {
            if remaining < line.tokens.len() {
                return Some(&mut line.tokens[remaining]);
            }
            remaining -= line.tokens.len();
        }
        None
    }

    /// `(line_index, index_in_line, flat_index)` for a flat token index.
    pub fn locate(&self, flat: usize) -> Option<(usize, usize, usize)> {
        let mut offset = 0;
        for (li, line) in self.lines.iter().enumerate() {
            if flat < offset + line.tokens.len() {
                return Some((li, flat - offset, flat));
            }
            offset += line.tokens.len();
        }
        None
    }

    pub fn flat_index(&self, line: usize, index: usize) -> Option<usize> {
        if line > self.lines.len() {
            return None;
        }
        let before: usize = self.lines.iter().take(line).map(|l| l.tokens.len()).sum();
        (index < self.lines.get(line)?.tokens.len()).then_some(before + index)
    }

    /// Pre-roll added before the audio starts.
    pub fn preroll_ms(&self) -> u64 {
        (self.style.intro_preroll_secs.clamp(0.0, MAX_PREROLL_SECS) as f64 * 1000.0).round() as u64
    }

    /// Visible timeline timestamp (stored audio time + tap offset + pre-roll).
    pub fn effective(&self, raw: u64) -> u64 {
        (raw as i64 + self.tap_offset_ms + self.preroll_ms() as i64).max(0) as u64
    }

    /// Effective start/end for every token, computed in one pass.
    ///
    /// A line's explicit `end_ms` (e.g. an LRC blank marker) is authoritative.
    /// Otherwise the line ends at the next line's start when that is close, or at
    /// the project tail after the final syllable. Token ends are always clamped to
    /// the line end, so a line before a long instrumental gap cannot stretch
    /// across it.
    pub fn spans(&self) -> Vec<Option<(u64, u64)>> {
        let intervals = self.line_intervals();
        let mut line_ends: Vec<Option<u64>> = vec![None; self.lines.len()];
        for interval in &intervals {
            line_ends[interval.index] = Some(interval.end);
        }

        let mut out = Vec::with_capacity(self.token_count());
        for (line_index, line) in self.lines.iter().enumerate() {
            let Some(line_end) = line_ends[line_index] else {
                out.extend(std::iter::repeat_n(None, line.tokens.len()));
                continue;
            };
            for (index, token) in line.tokens.iter().enumerate() {
                let Some(raw_start) = token.start_ms else {
                    out.push(None);
                    continue;
                };
                let start = self.effective(raw_start);
                let explicit_end = token.end_ms.map(|end| self.effective(end));
                let next_start = line
                    .tokens
                    .get(index + 1)
                    .and_then(|next| next.start_ms)
                    .map(|next| self.effective(next));
                let mut end = explicit_end
                    .filter(|end| *end > start)
                    .or(next_start.filter(|next| *next > start))
                    .unwrap_or(line_end)
                    .min(line_end);
                end = end.max(start + 10);
                out.push(Some((start, end)));
            }
        }
        out
    }

    pub fn token_span(&self, flat: usize) -> Option<(u64, u64)> {
        self.spans().get(flat).copied().flatten()
    }

    /// Effective start/end for every timed lyric line, in line order.
    ///
    /// `end_ms` on a line (or a token) wins over the inferred end; inferred ends
    /// are capped so the final syllable does not stretch across an instrumental
    /// solo.
    pub fn line_intervals(&self) -> Vec<LineInterval> {
        let mut starts: Vec<Option<u64>> = Vec::with_capacity(self.lines.len());
        let mut last_starts: Vec<Option<u64>> = Vec::with_capacity(self.lines.len());
        let mut last_explicit_ends: Vec<Option<u64>> = Vec::with_capacity(self.lines.len());

        for line in &self.lines {
            let mut first = None;
            let mut last = None;
            let mut last_explicit = None;
            for token in &line.tokens {
                if let Some(raw) = token.start_ms {
                    let start = self.effective(raw);
                    first.get_or_insert(start);
                    last = Some(start);
                    last_explicit = token.end_ms.map(|end| self.effective(end));
                }
            }
            starts.push(first);
            last_starts.push(last);
            last_explicit_ends.push(last_explicit);
        }

        let tail = self.tail_ms.max(50);
        let tail_cap = tail.saturating_mul(2);
        let mut intervals = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            let Some(start) = starts[index] else {
                continue;
            };
            let next_start = starts.iter().skip(index + 1).flatten().copied().next();
            let explicit_line_end = line
                .end_ms
                .map(|end| self.effective(end))
                .filter(|end| *end > start);

            let mut end = if let Some(explicit) = explicit_line_end {
                explicit
            } else {
                let last_start = last_starts[index].unwrap_or(start);
                let inferred = if let Some(next) = next_start {
                    if next.saturating_sub(last_start) <= tail_cap {
                        next
                    } else {
                        last_start + tail
                    }
                } else {
                    last_start + tail
                };
                inferred.max(last_explicit_ends[index].unwrap_or(0))
            };

            if let Some(next) = next_start {
                if next > start {
                    end = end.min(next);
                }
            }
            end = end.max(start + 10);
            intervals.push(LineInterval { index, start, end });
        }
        intervals
    }

    /// Active lyric line, using the inclusive interval `start <= t <= end`.
    pub fn active_line(&self, t: u64) -> Option<usize> {
        self.line_intervals()
            .into_iter()
            .filter(|interval| interval.start <= t && t <= interval.end)
            .max_by_key(|interval| interval.start)
            .map(|interval| interval.index)
    }

    /// First upcoming line that starts within `lead_ms` of `t`.
    pub fn on_deck_line(&self, t: u64, lead_ms: u64) -> Option<usize> {
        self.line_intervals()
            .into_iter()
            .filter(|interval| interval.start > t && interval.start - t <= lead_ms)
            .min_by_key(|interval| interval.start)
            .map(|interval| interval.index)
    }

    /// 4-step cue over the title-card pre-roll, if one is active.
    pub fn preroll_dots(&self, t: u64) -> Option<usize> {
        let preroll = self.preroll_ms();
        if preroll == 0 || t >= preroll {
            return None;
        }
        let phase = (t.saturating_mul(4) / preroll).min(3) as usize;
        Some(4 - phase)
    }

    /// First lyric start without the pre-roll (for the Auto button).
    pub fn first_lyric_start_ms(&self) -> Option<u64> {
        self.line_intervals()
            .first()
            .map(|interval| interval.start.saturating_sub(self.preroll_ms()))
    }

    /// Recommended pre-roll: 3.5 s if the opening line arrives in under 3.5 s.
    pub fn auto_preroll_secs(&self) -> f32 {
        match self.first_lyric_start_ms() {
            Some(start) if start < 3_500 => 3.5,
            _ => 0.0,
        }
    }

    /// Lead-in countdown for the upcoming line, if enabled and the gap qualifies.
    pub fn countdown_cue(&self, t: u64) -> Option<CountdownCue> {
        if !self.style.lead_in_countdown {
            return None;
        }
        let intervals = self.line_intervals();
        let next_position = intervals.iter().position(|interval| interval.start > t)?;
        // The title-card pre-roll has its own countdown; don't immediately run a
        // second one for the first lyric after the title card ends.
        if next_position == 0 && self.preroll_ms() > 0 {
            return None;
        }
        let next = *intervals.get(next_position)?;
        let previous_end = if next_position > 0 {
            intervals[next_position - 1].end
        } else {
            0
        };
        if next.start.saturating_sub(previous_end) < LEAD_IN_GAP_MS {
            return None;
        }
        let window_start = next.start.saturating_sub(COUNTDOWN_WINDOW_MS);
        if t < window_start || t >= next.start {
            return None;
        }
        let step = ((t - window_start) / COUNTDOWN_STEP_MS).min(3) as usize;
        Some(CountdownCue {
            line: next.index,
            dots: 4 - step,
            next_start: next.start,
        })
    }

    pub fn line_span(&self, line: usize) -> Option<(u64, u64)> {
        self.line_intervals()
            .into_iter()
            .find(|interval| interval.index == line)
            .map(|interval| (interval.start, interval.end))
    }

    pub fn set_token_start(&mut self, flat: usize, raw: u64) {
        if let Some(token) = self.token_mut(flat) {
            token.start_ms = Some(raw);
            // Ends are derived from the following token; drop stale overrides.
            token.end_ms = None;
        }
    }

    pub fn clear_token_start(&mut self, flat: usize) {
        if let Some(token) = self.token_mut(flat) {
            token.start_ms = None;
            token.end_ms = None;
        }
    }

    /// Move a token boundary by `delta_ms`, preserving an explicit end if present.
    pub fn nudge_token(&mut self, flat: usize, delta_ms: i64) {
        if let Some(token) = self.token_mut(flat) {
            let Some(raw) = token.start_ms else {
                return;
            };
            let raw = (raw as i64 + delta_ms).max(0) as u64;
            token.start_ms = Some(raw);
            if let Some(end) = token.end_ms {
                let end = (end as i64 + delta_ms).max(raw as i64 + 10) as u64;
                token.end_ms = Some(end);
            }
        }
    }

    pub fn set_token_end(&mut self, flat: usize, raw: Option<u64>) {
        if let Some(token) = self.token_mut(flat) {
            token.end_ms = raw;
        }
    }

    /// Problems that would make a render unusable. Empty means good to go.
    pub fn validate_for_render(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.instrumental.is_none() {
            issues.push("Load an instrumental track first.".to_owned());
        } else if let Some(path) = &self.instrumental {
            if !path.exists() {
                issues.push(format!("Instrumental file is missing: {}", path.display()));
            }
        }
        if self.token_count() == 0 {
            issues.push("Paste some lyrics and tap at least one word.".to_owned());
        } else {
            let untimed = self.token_count() - self.timed_count();
            if untimed > 0 {
                issues.push(format!(
                    "{untimed} of {} tokens still need a tap timestamp.",
                    self.token_count()
                ));
            }
            let mut previous: Option<u64> = None;
            let mut out_of_order = 0;
            for span in self.spans().into_iter().flatten() {
                if let Some(prev) = previous {
                    if span.0 < prev {
                        out_of_order += 1;
                    }
                }
                previous = Some(span.0);
            }
            if out_of_order > 0 {
                issues.push(format!(
                    "{out_of_order} token start times are out of order; nudge them into sequence."
                ));
            }
        }
        match &self.style.background {
            Background::Cover { path, .. } | Background::Video { path, .. } => {
                if let Some(path) = path {
                    if !path.exists() {
                        issues.push(format!("Background file is missing: {}", path.display()));
                    }
                }
            }
            Background::Gradient { .. } => {}
        }
        issues
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("Could not serialize project: {e}"))
    }

    pub fn save_json(&self, path: &Path) -> Result<(), String> {
        let json = self.to_json()?;
        crate::util::write_file(path, json.as_bytes())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let mut project: Self =
            serde_json::from_str(text).map_err(|e| format!("Invalid project JSON: {e}"))?;
        if project.version > PROJECT_VERSION {
            return Err(format!(
                "This project was made with a newer KaraokeKit (format v{}).",
                project.version
            ));
        }
        project.version = PROJECT_VERSION;
        project.normalize();
        Ok(project)
    }

    pub fn load_json(path: &Path) -> Result<Self, String> {
        let text = crate::util::read_to_string(path)?;
        Self::from_json(&text)
    }

    fn normalize(&mut self) {
        self.tail_ms = self.tail_ms.clamp(50, 10_000);
        self.style.font_size = self.style.font_size.clamp(12.0, 240.0);
        self.style.outline_width = self.style.outline_width.clamp(0.0, 40.0);
        self.style.shadow_width = self.style.shadow_width.clamp(0.0, 40.0);
        self.style.margin_v = self.style.margin_v.clamp(0.0, 500.0);
        self.style.intro_preroll_secs =
            (self.style.intro_preroll_secs.clamp(0.0, MAX_PREROLL_SECS) * 2.0).round() / 2.0;
        self.export.fps = if self.export.fps >= 60 { 60 } else { 30 };
        self.export.crf = self.export.crf.clamp(10, 40);
        if !matches!(self.export.preset.as_str(), "veryfast" | "medium" | "slow") {
            self.export.preset = "medium".to_owned();
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricLine {
    pub tokens: Vec<Token>,
    /// Optional explicit line end (e.g. a blank LRC timestamp marker).
    pub end_ms: Option<u64>,
}

impl LyricLine {
    pub fn text(&self) -> String {
        crate::lyrics::join_tokens(&self.tokens)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Token {
    pub text: String,
    /// True when this syllable begins a whitespace-separated word.
    pub word_start: bool,
    pub start_ms: Option<u64>,
    /// Optional explicit end. Normally `None`: the end is the next token's start.
    pub end_ms: Option<u64>,
}

impl Default for Token {
    fn default() -> Self {
        Self {
            text: String::new(),
            word_start: true,
            start_ms: None,
            end_ms: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub font_family: String,
    pub font_size: f32,
    pub base_color: Rgb,
    pub highlight_color: Rgb,
    pub outline_width: f32,
    pub shadow_width: f32,
    pub margin_v: f32,
    pub background: Background,
    /// Show a 4-dot cue before lines that follow a multi-second instrumental gap.
    #[serde(default = "default_true")]
    pub lead_in_countdown: bool,
    /// Seconds of title-card lead-in prepended before the audio starts.
    #[serde(default)]
    pub intro_preroll_secs: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            font_family: default_font(),
            font_size: default_size(),
            base_color: default_base(),
            highlight_color: default_highlight(),
            outline_width: default_outline(),
            shadow_width: default_shadow(),
            margin_v: default_margin_v(),
            background: Background::default(),
            lead_in_countdown: true,
            intro_preroll_secs: 0.0,
        }
    }
}

impl Style {
    /// Brightness multiplier for the upcoming lyric line. Shared by the preview
    /// and the `.ass` exporter so both match.
    pub const NEXT_LINE_BRIGHTNESS: f32 = 0.55;

    pub fn next_line_color(&self) -> Rgb {
        let dim = |channel: u8| (channel as f32 * Self::NEXT_LINE_BRIGHTNESS).round() as u8;
        [
            dim(self.base_color[0]),
            dim(self.base_color[1]),
            dim(self.base_color[2]),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Background {
    Gradient {
        #[serde(default = "default_gradient_top")]
        top: Rgb,
        #[serde(default = "default_gradient_bottom")]
        bottom: Rgb,
    },
    Cover {
        #[serde(default)]
        path: Option<PathBuf>,
        #[serde(default = "default_blur")]
        blur: f32,
        #[serde(default = "default_dim")]
        dim: f32,
    },
    Video {
        #[serde(default)]
        path: Option<PathBuf>,
        #[serde(default = "default_dim")]
        dim: f32,
    },
}

fn default_gradient_top() -> Rgb {
    [11, 16, 32]
}
fn default_gradient_bottom() -> Rgb {
    [27, 15, 43]
}

impl Default for Background {
    fn default() -> Self {
        Self::Gradient {
            top: default_gradient_top(),
            bottom: default_gradient_bottom(),
        }
    }
}

impl Background {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Gradient { .. } => "Gradient",
            Self::Cover { .. } => "Cover image",
            Self::Video { .. } => "Background video",
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Gradient { .. } => None,
            Self::Cover { path, .. } | Self::Video { path, .. } => path.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Mp4,
    Mkv,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
        }
    }

    /// FFmpeg muxer name (`.mkv` maps to the `matroska` muxer).
    pub fn ffmpeg_format(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "matroska",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4 (H.264 + AAC)",
            Self::Mkv => "MKV (H.264 + FLAC/AAC)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Aac,
    Flac,
}

impl AudioCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::Aac => "AAC 320k",
            Self::Flac => "FLAC (lossless)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AspectRatio {
    #[default]
    #[serde(rename = "16:9")]
    Widescreen,
    #[serde(rename = "4:3")]
    Classic,
}

impl AspectRatio {
    pub const fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Widescreen => (1920, 1080),
            Self::Classic => (1440, 1080),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Widescreen => "16:9 Widescreen",
            Self::Classic => "4:3 Classic",
        }
    }

    pub fn ratio(self) -> f32 {
        let (width, height) = self.dimensions();
        width as f32 / height as f32
    }

    /// Side margin matching the preview's 86% text-wrap width.
    pub fn side_margin(self) -> u32 {
        let (width, _) = self.dimensions();
        (width as f32 * 0.07).round() as u32
    }

    pub fn wrap_width(self) -> u32 {
        let (width, _) = self.dimensions();
        width.saturating_sub(self.side_margin() * 2)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub aspect: AspectRatio,
    pub container: Container,
    pub fps: u32,
    pub audio_codec: AudioCodec,
    pub crf: u8,
    pub preset: String,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            aspect: AspectRatio::Widescreen,
            container: Container::Mp4,
            fps: 30,
            audio_codec: AudioCodec::Aac,
            crf: 18,
            preset: "medium".to_owned(),
        }
    }
}

impl ExportSettings {
    /// MP4 cannot carry FLAC in this pipeline, so it always uses AAC.
    pub fn effective_codec(&self) -> AudioCodec {
        match self.container {
            Container::Mp4 => AudioCodec::Aac,
            Container::Mkv => self.audio_codec,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_project() -> Project {
        let mut p = Project {
            title: "Cafeteria Song".to_owned(),
            instrumental: Some(PathBuf::from("winter_wrap_up.flac")),
            ..Project::default()
        };
        p.lines = vec![
            LyricLine {
                tokens: vec![
                    Token {
                        text: "Trays".into(),
                        word_start: true,
                        start_ms: Some(4_200),
                        end_ms: None,
                    },
                    Token {
                        text: "are".into(),
                        word_start: true,
                        start_ms: Some(4_700),
                        end_ms: None,
                    },
                ],
                end_ms: None,
            },
            LyricLine {
                tokens: vec![Token {
                    text: "clat".into(),
                    word_start: true,
                    start_ms: Some(5_200),
                    end_ms: None,
                }],
                end_ms: None,
            },
        ];
        p
    }

    #[test]
    fn uses_next_start_as_end() {
        let p = sample_project();
        assert_eq!(p.token_span(0), Some((4_200, 4_700)));
        assert_eq!(p.token_span(1), Some((4_700, 5_200)));
        assert_eq!(p.token_span(2), Some((5_200, 5_200 + DEFAULT_TAIL_MS)));
    }

    #[test]
    fn offset_shifts_every_visible_timestamp() {
        let mut p = sample_project();
        p.tap_offset_ms = -120;
        assert_eq!(p.token_span(0), Some((4_080, 4_580)));
        p.tap_offset_ms = 500;
        assert_eq!(p.token_span(0), Some((4_700, 5_200)));
        p.tap_offset_ms = -2_000;
        assert_eq!(p.token_span(0), Some((2_200, 2_700)));
    }

    #[test]
    fn nudge_moves_the_boundary() {
        let mut p = sample_project();
        p.nudge_token(1, -25);
        assert_eq!(p.token_span(0), Some((4_200, 4_675)));
        assert_eq!(p.token_span(1), Some((4_675, 5_200)));
        p.nudge_token(2, 25);
        assert_eq!(p.token_span(2), Some((5_225, 5_225 + DEFAULT_TAIL_MS)));
    }

    #[test]
    fn line_final_syllables_do_not_stretch_across_long_gaps() {
        let mut project = Project {
            tail_ms: 900,
            lines: vec![
                LyricLine {
                    tokens: vec![Token {
                        text: "hold".into(),
                        word_start: true,
                        start_ms: Some(1_000),
                        end_ms: None,
                    }],
                    end_ms: None,
                },
                LyricLine {
                    tokens: vec![Token {
                        text: "next".into(),
                        word_start: true,
                        start_ms: Some(6_000),
                        end_ms: None,
                    }],
                    end_ms: None,
                },
            ],
            ..Project::default()
        };
        assert_eq!(project.token_span(0), Some((1_000, 1_900)));

        // A nearby next line still provides the natural end.
        project.lines[1].tokens[0].start_ms = Some(2_200);
        assert_eq!(project.token_span(0), Some((1_000, 2_200)));
    }

    #[test]
    fn explicit_tail_is_respected() {
        let mut p = sample_project();
        p.set_token_end(2, Some(6_500));
        assert_eq!(p.token_span(2), Some((5_200, 6_500)));
    }

    #[test]
    fn tracks_flat_indices_across_lines() {
        let p = sample_project();
        assert_eq!(p.locate(2), Some((1, 0, 2)));
        assert_eq!(p.flat_index(1, 0), Some(2));
        assert_eq!(p.first_untimed(), None);
        assert_eq!(p.line_span(0), Some((4_200, 5_200)));
    }

    #[test]
    fn validation_flags_untimed_tokens() {
        let mut p = sample_project();
        p.token_mut(1).unwrap().start_ms = None;
        let issues = p.validate_for_render();
        assert!(issues.iter().any(|m| m.contains("need a tap")));
    }

    #[test]
    fn aspect_defaults_metrics_and_round_trip() {
        let project = Project::default();
        assert_eq!(project.export.aspect, AspectRatio::Widescreen);

        assert_eq!(AspectRatio::Widescreen.dimensions(), (1920, 1080));
        assert_eq!(AspectRatio::Widescreen.side_margin(), 134);
        assert_eq!(AspectRatio::Widescreen.wrap_width(), 1652);
        assert_eq!(AspectRatio::Classic.dimensions(), (1440, 1080));
        assert_eq!(AspectRatio::Classic.side_margin(), 101);
        assert_eq!(AspectRatio::Classic.wrap_width(), 1238);

        let mut classic = Project::default();
        classic.export.aspect = AspectRatio::Classic;
        let json = classic.to_json().unwrap();
        assert!(json.contains("\"aspect\": \"4:3\""));
        assert_eq!(
            Project::from_json(&json).unwrap().export.aspect,
            AspectRatio::Classic
        );

        // Older projects have no aspect field and must default to 16:9.
        let old = Project::from_json("{\"title\": \"Old project\"}").unwrap();
        assert_eq!(old.export.aspect, AspectRatio::Widescreen);
    }

    fn countdown_project(gap_ms: u64) -> Project {
        Project {
            lines: vec![
                LyricLine {
                    tokens: vec![Token {
                        text: "first".into(),
                        word_start: true,
                        start_ms: Some(1_000),
                        end_ms: None,
                    }],
                    end_ms: Some(6_000),
                },
                LyricLine {
                    tokens: vec![Token {
                        text: "second".into(),
                        word_start: true,
                        start_ms: Some(6_000 + gap_ms),
                        end_ms: None,
                    }],
                    end_ms: None,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn lead_in_countdown_steps_four_three_two_one() {
        let project = countdown_project(12_000);
        assert_eq!(project.countdown_cue(14_799), None);
        assert_eq!(project.countdown_cue(14_800).map(|c| c.dots), Some(4));
        assert_eq!(project.countdown_cue(15_600).map(|c| c.dots), Some(3));
        assert_eq!(project.countdown_cue(16_400).map(|c| c.dots), Some(2));
        assert_eq!(project.countdown_cue(17_200).map(|c| c.dots), Some(1));
        assert_eq!(project.countdown_cue(17_999).map(|c| c.dots), Some(1));
        assert_eq!(project.countdown_cue(18_000), None);
        assert_eq!(project.countdown_cue(14_800).map(|c| c.line), Some(1));
    }

    #[test]
    fn countdown_requires_a_four_second_gap_and_is_toggleable() {
        let short = countdown_project(3_900);
        assert_eq!(short.countdown_cue(6_700), None);

        let mut disabled = countdown_project(12_000);
        disabled.style.lead_in_countdown = false;
        assert_eq!(disabled.countdown_cue(16_000), None);

        // A first line at >= 4 s also gets a countdown.
        let first = Project {
            lines: vec![LyricLine {
                tokens: vec![Token {
                    text: "opening".into(),
                    word_start: true,
                    start_ms: Some(5_000),
                    end_ms: None,
                }],
                end_ms: None,
            }],
            ..Default::default()
        };
        assert_eq!(first.countdown_cue(1_800).map(|c| c.dots), Some(4));
        assert_eq!(first.countdown_cue(0), None);

        assert_eq!(countdown_text(4), "● ● ● ●");
        assert_eq!(countdown_text(1), "●");
    }

    #[test]
    fn preroll_shifts_visible_time_without_touching_tokens() {
        let mut project = Project {
            title: "Winter Wrap Up".to_owned(),
            artist: "The Weather Team".to_owned(),
            lines: vec![LyricLine {
                tokens: vec![Token {
                    text: "win".into(),
                    word_start: true,
                    start_ms: Some(0),
                    end_ms: None,
                }],
                end_ms: None,
            }],
            ..Default::default()
        };
        project.style.intro_preroll_secs = 3.5;

        assert_eq!(project.preroll_ms(), 3_500);
        assert_eq!(project.effective(0), 3_500);
        assert_eq!(project.line_span(0).map(|span| span.0), Some(3_500));
        assert_eq!(project.lines[0].tokens[0].start_ms, Some(0));
        assert_eq!(project.first_lyric_start_ms(), Some(0));
        assert_eq!(project.auto_preroll_secs(), 3.5);

        assert_eq!(project.preroll_dots(0), Some(4));
        assert_eq!(project.preroll_dots(874), Some(4));
        assert_eq!(project.preroll_dots(875), Some(3));
        assert_eq!(project.preroll_dots(1_750), Some(2));
        assert_eq!(project.preroll_dots(2_625), Some(1));
        assert_eq!(project.preroll_dots(3_499), Some(1));
        assert_eq!(project.preroll_dots(3_500), None);
    }

    #[test]
    fn preroll_defaults_off_and_auto_is_conservative() {
        let mut project = Project::default();
        assert_eq!(project.preroll_ms(), 0);
        assert_eq!(project.auto_preroll_secs(), 0.0);

        project.lines = vec![LyricLine {
            tokens: vec![Token {
                text: "late".into(),
                word_start: true,
                start_ms: Some(5_000),
                end_ms: None,
            }],
            end_ms: None,
        }];
        assert_eq!(project.auto_preroll_secs(), 0.0);

        project.lines[0].tokens[0].start_ms = Some(1_000);
        assert_eq!(project.auto_preroll_secs(), 3.5);

        let old = Project::from_json("{\"style\": {\"margin_v\": 90.0}}").unwrap();
        assert_eq!(old.style.intro_preroll_secs, 0.0);

        project.style.intro_preroll_secs = 3.25;
        let json = project.to_json().unwrap();
        let back = Project::from_json(&json).unwrap();
        assert_eq!(back.style.intro_preroll_secs, 3.5);
    }

    #[test]
    fn lead_in_setting_defaults_on_and_round_trips() {
        let mut project = Project::default();
        assert!(project.style.lead_in_countdown);
        let old = Project::from_json("{\"style\": {\"font_size\": 80.0}}").unwrap();
        assert!(old.style.lead_in_countdown);

        project.style.lead_in_countdown = false;
        let json = project.to_json().unwrap();
        assert!(json.contains("\"lead_in_countdown\": false"));
        assert!(!Project::from_json(&json).unwrap().style.lead_in_countdown);
    }

    #[test]
    fn upcoming_line_colour_is_dimmed() {
        let style = Style::default();
        assert_eq!(style.next_line_color(), [140, 140, 140]);
    }

    #[test]
    fn json_round_trip() {
        let p = sample_project();
        let json = p.to_json().unwrap();
        let back = Project::from_json(&json).unwrap();
        assert_eq!(p, back);
    }
}
