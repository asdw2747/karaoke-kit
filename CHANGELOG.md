# Changelog

## 1.0.0 — 2026-11-14

First public release.

### Features

- Tap-to-sync lyric editor with per-syllable timestamps, keyboard-driven tapping,
  undo, nudging, and a live progressive-sweep preview.
- 16:9 (1920×1080) and 4:3 (1440×1080) canvases with configurable typography,
  outline, shadow, margins, and highlight/sweep colors.
- Lead-in countdown: a 4→3→2→1 dot cue before lines that follow a long
  instrumental gap.
- Title-card pre-roll (0–6 s): title/artist card, opening line on deck, and a
  4-step countdown before the audio starts; lyrics, preview, and export stay in
  sync while stored tap timestamps remain unchanged.
- Backgrounds: gradient, cover image (blur/dim), or looping video (dim).
- MP4 (H.264/AAC) and MKV (H.264/AAC or FLAC) export through FFmpeg, with
  progress, speed, and cancellation.
- LRC import/export (plain and enhanced word timestamps, metadata, explicit end
  markers). LRC export is audio-relative and accounts for the title-card pre-roll.
- ASS subtitle export, including title-card, countdown, and on-deck events.
- Headless rendering CLI: `karaoke-kit --render project.json --out song.mp4`.
- Linux and Windows release builds; the Windows binary statically links the
  MSVC runtime and reattaches to the parent console for CLI output.

### Fixes

- LRC export no longer includes the title-card pre-roll, and re-importing an LRC
  no longer shifts timings twice.
- Added unsaved-changes confirmation before New, Open, and window close.
- Added third-party license notices for bundled fonts and dependencies.
- Documented extreme-font title-card crowding and audio-relative LRC timestamps.

### Known limitations

- FFmpeg must be installed and on `PATH` (or set `KARAOKEKIT_FFMPEG`) to render.
- Background video is looped for the whole song; the editor preview shows a still
  frame.
- The bundled font covers Latin/Greek/Cyrillic and many symbols; choose a system
  font for other scripts.
