//! Small shared helpers: time formatting, colour conversion, temp dirs, ffmpeg lookup.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// `mm:ss.cc` (centiseconds), as used throughout the UI and in project files.
pub fn format_time(ms: u64) -> String {
    let cs = ms / 10;
    let minutes = cs / 6000;
    let seconds = (cs / 100) % 60;
    let centis = cs % 100;
    format!("{minutes:02}:{seconds:02}.{centis:02}")
}

/// Compact duration such as `3.4s`, used in status messages.
pub fn format_duration_short(ms: u64) -> String {
    let secs = ms as f64 / 1000.0;
    if secs >= 60.0 {
        format!("{}:{:04.1}", (secs / 60.0).floor() as u64, secs % 60.0)
    } else {
        format!("{secs:.1}s")
    }
}

/// Convert an ASS colour (`BBGGRR`) into the UI colour triple.
pub fn rgb_to_ass(rgb: [u8; 3]) -> String {
    // ASS stores colours as &HAABBGGRR; alpha 00 means fully opaque.
    format!("&H00{:02X}{:02X}{:02X}", rgb[2], rgb[1], rgb[0])
}

pub fn rgb_to_hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// A monotonic-enough unique suffix for temporary directories.
pub fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{n}", std::process::id())
}

/// Create a fresh directory below the system temp dir.
pub fn temp_dir(prefix: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", unique_suffix()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Path to the ffmpeg executable. `KARAOKEKIT_FFMPEG` can override PATH lookup.
pub fn ffmpeg_path() -> String {
    std::env::var("KARAOKEKIT_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned())
}

/// Check that ffmpeg exists and can start. Returns its first version line.
pub fn probe_ffmpeg() -> Result<String, String> {
    let mut command = std::process::Command::new(ffmpeg_path());
    command
        .arg("-hide_banner")
        .arg("-version")
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().map_err(|e| {
        format!(
            "Could not run `{}`: {e}. Install FFmpeg and make sure it is on PATH, \
             or set KARAOKEKIT_FFMPEG to the full executable path.",
            ffmpeg_path()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "`{} -version` failed with {}.",
            ffmpeg_path(),
            output.status
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .lines()
        .next()
        .unwrap_or("ffmpeg (unknown version)")
        .to_owned())
}

/// Write bytes to a file, creating parent directories as needed.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_to_string(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Basic filename (without extension) used for default export names.
pub fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "karaoke".to_owned())
}

/// Make a user-visible title safe for a filename.
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')' | '\'') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "karaoke".to_owned()
    } else {
        trimmed.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_centisecond_timestamps() {
        assert_eq!(format_time(0), "00:00.00");
        assert_eq!(format_time(1_234), "00:01.23");
        assert_eq!(format_time(61_230), "01:01.23");
        assert_eq!(format_time(3_600_000), "60:00.00");
    }

    #[test]
    fn ass_colour_is_bbggrr() {
        assert_eq!(rgb_to_ass([0xFF, 0xD5, 0x4A]), "&H004AD5FF");
    }

    #[test]
    fn sanitizes_titles() {
        assert_eq!(sanitize_filename("Winter Wrap-Up!"), "Winter Wrap-Up_");
        assert_eq!(sanitize_filename("   "), "karaoke");
    }
}
