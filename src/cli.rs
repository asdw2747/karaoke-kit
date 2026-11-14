//! Headless rendering: `karaoke-kit --render project.json --out song.mp4`.
//!
//! Shares the exact ASS/background/FFmpeg pipeline as the GUI, which makes
//! automated verification and batch rendering possible.

use crate::export;
use crate::model::{AudioCodec, Container, Project};
use std::path::PathBuf;

pub fn run() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return None;
    }
    match args[0].as_str() {
        "--version" | "-V" => {
            println!("KaraokeKit {}", env!("CARGO_PKG_VERSION"));
            Some(0)
        }
        "--help" | "-h" => {
            print_help();
            Some(0)
        }
        "--render" => Some(render(&args[1..])),
        other => {
            eprintln!("Unknown argument: {other}");
            eprintln!("Run `karaoke-kit --help` for usage.");
            Some(2)
        }
    }
}

fn print_help() {
    println!(
        "KaraokeKit {} — tap-to-sync karaoke lyric video creator\n\
         \n\
         USAGE:\n\
           karaoke-kit                         Launch the desktop editor\n\
           karaoke-kit --render <project.json> --out <video> [options]\n\
           karaoke-kit --version\n\
         \n\
         RENDER OPTIONS:\n\
           --out <path>        Output video path (required)\n\
           --format <mp4|mkv>  Container (default: from the project)\n\
           --fps <30|60>       Frame rate (default: from the project)\n\
           --codec <aac|flac>  MKV audio codec (MP4 always uses AAC 320k)\n\
           --crf <14-28>       H.264 quality (default: from the project)\n\
           --preset <name>     x264 preset (veryfast/medium/slow)\n\
         \n\
         FFmpeg must be installed and on PATH, or set KARAOKEKIT_FFMPEG.",
        env!("CARGO_PKG_VERSION")
    );
}

fn render(args: &[String]) -> i32 {
    let mut project_path: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut format: Option<Container> = None;
    let mut fps: Option<u32> = None;
    let mut codec: Option<AudioCodec> = None;
    let mut crf: Option<u8> = None;
    let mut preset: Option<String> = None;

    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let mut value = |name: &str| -> Result<String, String> {
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        let result: Result<(), String> = (|| {
            match arg {
                "--out" | "-o" => output = Some(PathBuf::from(value("--out")?)),
                "--format" | "-f" => {
                    format = Some(match value("--format")?.to_ascii_lowercase().as_str() {
                        "mp4" => Container::Mp4,
                        "mkv" => Container::Mkv,
                        other => return Err(format!("Unsupported format `{other}` (mp4 or mkv).")),
                    })
                }
                "--fps" => {
                    fps = Some(match value("--fps")?.as_str() {
                        "30" => 30,
                        "60" => 60,
                        other => return Err(format!("Unsupported fps `{other}` (30 or 60).")),
                    })
                }
                "--codec" | "-c" => {
                    codec = Some(match value("--codec")?.to_ascii_lowercase().as_str() {
                        "aac" => AudioCodec::Aac,
                        "flac" => AudioCodec::Flac,
                        other => return Err(format!("Unsupported codec `{other}` (aac or flac).")),
                    })
                }
                "--crf" => {
                    let parsed: u8 = value("--crf")?
                        .parse()
                        .map_err(|_| "--crf expects a number 14-28.".to_owned())?;
                    if !(14..=28).contains(&parsed) {
                        return Err("--crf must be between 14 and 28.".to_owned());
                    }
                    crf = Some(parsed);
                }
                "--preset" => preset = Some(value("--preset")?),
                other if other.starts_with('-') => {
                    return Err(format!("Unknown option `{other}`."))
                }
                other => {
                    if project_path.is_some() {
                        return Err("Only one project file can be rendered at a time.".to_owned());
                    }
                    project_path = Some(PathBuf::from(other));
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("error: {error}");
            return 2;
        }
        index += 1;
    }

    let Some(project_path) = project_path else {
        eprintln!("error: no project file supplied (use --render <project.json>).");
        return 2;
    };
    let mut project = match Project::load_json(&project_path) {
        Ok(project) => project,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    if let Some(format) = format {
        project.export.container = format;
    }
    if let Some(fps) = fps {
        project.export.fps = fps;
    }
    if let Some(codec) = codec {
        project.export.audio_codec = codec;
    }
    if let Some(crf) = crf {
        project.export.crf = crf;
    }
    if let Some(preset) = preset {
        project.export.preset = preset;
    }
    if project.export.container == Container::Mp4 {
        project.export.audio_codec = AudioCodec::Aac;
    }

    let Some(mut output) = output else {
        eprintln!("error: --out <video> is required.");
        return 2;
    };
    let extension = project.export.container.extension();
    if output.extension().and_then(|e| e.to_str()) != Some(extension) {
        output.set_extension(extension);
    }

    let issues = project.validate_for_render();
    if !issues.is_empty() {
        eprintln!("error: the project is not ready to render:");
        for issue in issues {
            eprintln!("  - {issue}");
        }
        return 1;
    }

    let instrumental = match &project.instrumental {
        Some(path) => path.clone(),
        None => {
            eprintln!("error: no instrumental track in the project.");
            return 1;
        }
    };
    eprintln!("Probing duration…");
    let duration = match crate::audio::probe_duration(&instrumental) {
        Ok(duration) => duration,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };

    println!(
        "Rendering {} → {}",
        project_path.display(),
        output.display()
    );
    let plan = match export::prepare(&project, &output, duration) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let result = export::render_blocking(plan, |fraction, speed| {
        let percent = fraction * 100.0;
        match speed {
            Some(speed) => print!("\r{percent:5.1}%  {speed}      "),
            None => print!("\r{percent:5.1}%            "),
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());
    });
    println!();
    match result {
        Ok(()) => {
            println!("Done: {}", output.display());
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}
