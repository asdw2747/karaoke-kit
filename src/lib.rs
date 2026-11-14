//! KaraokeKit — tap-to-sync karaoke lyric video creator.
//!
//! The library exposes the project model, lyric/LRC/ASS pipeline, audio engine,
//! FFmpeg export, and the egui application so the GUI binary and integration
//! tests share exactly the same implementation.

pub mod ass;
pub mod assets;
pub mod audio;
pub mod cli;
pub mod export;
pub mod icon;
pub mod lrc;
pub mod lyrics;
pub mod model;
pub mod preview;
pub mod theme;
pub mod timeline;
pub mod util;

pub mod app;

pub use app::KaraokeKitApp;
