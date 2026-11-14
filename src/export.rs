//! FFmpeg render pipeline: background preparation, `.ass` burning, progress, cancel.
//!
//! FFmpeg runs with `current_dir` set to a temporary work dir so the `ass` filter
//! only ever sees relative paths (`karaoke.ass`, `fonts`). That sidesteps filtergraph
//! escaping bugs with Windows drive letters and possessive apostrophes.

use crate::ass::build_ass;
use crate::model::{AudioCodec, Container, Project};
use crate::util;
use image::{Rgb, RgbImage};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
pub struct RenderPlan {
    pub args: Vec<String>,
    pub workdir: PathBuf,
    pub duration_ms: u64,
    pub output: PathBuf,
}

#[derive(Debug, Clone)]
pub enum RenderEvent {
    Progress {
        fraction: f32,
        speed: Option<String>,
    },
    Done,
    Failed(String),
    Cancelled,
}

pub struct RenderHandle {
    pub events: Receiver<RenderEvent>,
    pub cancel: Arc<AtomicBool>,
    pub output: PathBuf,
}

impl RenderHandle {
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_owned())
    }
}

/// Build the ffmpeg argument list. Pure so it can be unit tested.
fn build_args(project: &Project, plan: &RenderPlan, background_input: &Path) -> Vec<String> {
    let settings = &project.export;
    let (width, height) = settings.aspect.dimensions();
    let fps = settings.fps.clamp(1, 120);
    let instrumental = project
        .instrumental
        .as_ref()
        .map(|p| absolutize(p).to_string_lossy().into_owned())
        .unwrap_or_default();
    let output = plan.output.to_string_lossy().into_owned();

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-y".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
    ];

    match &project.style.background {
        crate::model::Background::Video { .. } => {
            args.push("-stream_loop".into());
            args.push("-1".into());
            args.push("-i".into());
            args.push(background_input.to_string_lossy().into_owned());
        }
        _ => {
            args.push("-loop".into());
            args.push("1".into());
            args.push("-framerate".into());
            args.push(fps.to_string());
            args.push("-i".into());
            args.push(background_input.to_string_lossy().into_owned());
        }
    }
    args.push("-i".into());
    args.push(instrumental);

    let mut video_chain = format!(
        "[0:v]scale={width}:{height}:force_original_aspect_ratio=increase,\
         crop={width}:{height},setsar=1"
    );
    match &project.style.background {
        crate::model::Background::Gradient { .. } => {}
        crate::model::Background::Cover { blur, dim, .. } => {
            if *blur > 0.2 {
                video_chain.push_str(&format!(",gblur=sigma={:.2}", blur.clamp(0.0, 100.0)));
            }
            if *dim > 0.01 {
                video_chain.push_str(&format!(
                    ",eq=brightness=-{:.3}:contrast=1.02",
                    dim.clamp(0.0, 0.9)
                ));
            }
        }
        crate::model::Background::Video { dim, .. } => {
            video_chain.push_str(&format!(",fps={fps}"));
            if *dim > 0.01 {
                video_chain.push_str(&format!(
                    ",eq=brightness=-{:.3}:contrast=1.02",
                    dim.clamp(0.0, 0.9)
                ));
            }
        }
    }
    video_chain.push_str(",setsar=1,ass=karaoke.ass:fontsdir=fonts[v]");
    args.push("-filter_complex".into());
    args.push(video_chain);
    args.push("-map".into());
    args.push("[v]".into());
    args.push("-map".into());
    args.push("1:a".into());

    if project.preroll_ms() > 0 {
        args.push("-af".into());
        args.push(format!("adelay={}:all=1", project.preroll_ms()));
    }

    args.push("-t".into());
    args.push(format!("{:.3}", plan.duration_ms as f64 / 1000.0));
    args.push("-r".into());
    args.push(fps.to_string());

    args.push("-c:v".into());
    args.push("libx264".into());
    args.push("-preset".into());
    args.push(settings.preset.clone());
    args.push("-crf".into());
    args.push(settings.crf.clamp(10, 40).to_string());
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    args.push("-profile:v".into());
    args.push("high".into());

    let codec = settings.effective_codec();
    match codec {
        AudioCodec::Aac => {
            args.push("-c:a".into());
            args.push("aac".into());
            args.push("-b:a".into());
            args.push("320k".into());
        }
        AudioCodec::Flac => {
            args.push("-c:a".into());
            args.push("flac".into());
            args.push("-compression_level".into());
            args.push("8".into());
        }
    }

    if settings.container == Container::Mp4 {
        args.push("-movflags".into());
        args.push("+faststart".into());
    }
    if !project.title.trim().is_empty() {
        args.push("-metadata".into());
        args.push(format!("title={}", project.title.trim()));
    }
    if !project.artist.trim().is_empty() {
        args.push("-metadata".into());
        args.push(format!("artist={}", project.artist.trim()));
    }
    args.push("-f".into());
    args.push(settings.container.ffmpeg_format().into());
    args.push(output);
    args
}

/// Write the gradient background at the target resolution.
pub fn write_gradient_png(
    path: &Path,
    top: [u8; 3],
    bottom: [u8; 3],
    width: u32,
    height: u32,
) -> Result<(), String> {
    let mut image = RgbImage::new(width, height);
    let top = Rgb(top);
    let bottom = Rgb(bottom);
    let width = width as f32;
    let height = height as f32;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let t = ((y as f32 / height) * 0.88 + (x as f32 / width) * 0.12).clamp(0.0, 1.0);
        let dx = (x as f32 / width - 0.5) * 2.0;
        let dy = (y as f32 / height - 0.5) * 2.0;
        let vignette = 1.0 - 0.22 * (dx * dx + dy * dy).min(1.0);
        let blend = |a: f32, b: f32| ((a + (b - a) * t) * vignette).clamp(0.0, 255.0) as u8;
        *pixel = Rgb([
            blend(top.0[0] as f32, bottom.0[0] as f32),
            blend(top.0[1] as f32, bottom.0[1] as f32),
            blend(top.0[2] as f32, bottom.0[2] as f32),
        ]);
    }
    image
        .save(path)
        .map_err(|e| format!("Could not write background image: {e}"))
}

/// Prepare a render: generate ASS, font, gradient/cover inputs, and arguments.
pub fn prepare(project: &Project, output: &Path, duration_ms: u64) -> Result<RenderPlan, String> {
    let instrumental = project
        .instrumental
        .as_ref()
        .ok_or_else(|| "No instrumental track is loaded.".to_owned())?;
    if !instrumental.exists() {
        return Err(format!(
            "Instrumental file is missing: {}",
            instrumental.display()
        ));
    }
    if duration_ms == 0 {
        return Err("The audio duration is zero; load a valid stem.".to_owned());
    }
    let mut output = absolutize(output);
    if output.extension().and_then(|e| e.to_str()) != Some(project.export.container.extension()) {
        output.set_extension(project.export.container.extension());
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create output folder: {e}"))?;
    }

    let workdir = util::temp_dir("karaokekit-render")
        .map_err(|e| format!("Could not create a temporary render folder: {e}"))?;
    let ass = build_ass(project);
    util::write_file(&workdir.join("karaoke.ass"), ass.as_bytes())?;
    util::write_file(
        &workdir.join("fonts").join("DejaVuSans.ttf"),
        crate::assets::FONT_TTF,
    )?;

    let background = match &project.style.background {
        crate::model::Background::Gradient { top, bottom } => {
            let path = workdir.join("background.png");
            write_gradient_png(
                &path,
                *top,
                *bottom,
                project.export.aspect.dimensions().0,
                project.export.aspect.dimensions().1,
            )?;
            path
        }
        crate::model::Background::Cover {
            path: Some(path), ..
        } => absolutize(path),
        crate::model::Background::Cover { path: None, .. } => {
            return Err("Choose a cover image, or switch to the gradient background.".to_owned())
        }
        crate::model::Background::Video {
            path: Some(path), ..
        } => absolutize(path),
        crate::model::Background::Video { path: None, .. } => {
            return Err(
                "Choose a background video, or switch to the gradient background.".to_owned(),
            )
        }
    };
    if !background.exists() {
        return Err(format!(
            "Background file is missing: {}",
            background.display()
        ));
    }

    let mut plan = RenderPlan {
        args: Vec::new(),
        workdir,
        duration_ms: duration_ms.saturating_add(project.preroll_ms()),
        output,
    };
    plan.args = build_args(project, &plan, &background);
    Ok(plan)
}

fn parse_progress(line: &str) -> Option<(Option<f32>, Option<String>)> {
    let (key, value) = line.split_once('=')?;
    match key.trim() {
        "out_time_us" | "out_time_ms" => {
            let us: u64 = value.trim().parse().ok()?;
            Some((Some(us as f32 / 1_000_000.0), None))
        }
        "speed" => {
            let speed = value.trim();
            Some((
                None,
                (!speed.is_empty() && speed != "N/A").then(|| speed.to_owned()),
            ))
        }
        "progress" if value.trim() == "end" => Some((Some(1.0e9), None)),
        _ => None,
    }
}

fn spawn_worker(plan: RenderPlan, tx: Sender<RenderEvent>, cancel: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let result = run_worker(&plan, &tx, &cancel);
        let _ = tx.send(result);
    });
}

fn run_worker(plan: &RenderPlan, tx: &Sender<RenderEvent>, cancel: &AtomicBool) -> RenderEvent {
    let log_path = plan.workdir.join("ffmpeg.log");
    let log = match std::fs::File::create(&log_path) {
        Ok(file) => file,
        Err(e) => return RenderEvent::Failed(format!("Could not create render log: {e}")),
    };
    let mut command = Command::new(util::ffmpeg_path());
    command
        .args(&plan.args)
        .current_dir(&plan.workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&plan.workdir);
            return RenderEvent::Failed(format!(
                "Could not start `{}`: {e}. Install FFmpeg and ensure it is on PATH, \
                 or set KARAOKEKIT_FFMPEG.",
                util::ffmpeg_path()
            ));
        }
    };

    let stdout = child.stdout.take();
    let duration_s = (plan.duration_ms as f32 / 1000.0).max(0.001);
    let reader_tx = tx.clone();
    let reader = stdout.map(|stdout| {
        std::thread::spawn(move || {
            let mut speed: Option<String> = None;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if std::env::var_os("KARAOKEKIT_DEBUG_PROGRESS").is_some() {
                    eprintln!("progress> {line}");
                }
                if let Some((fraction, parsed_speed)) = parse_progress(&line) {
                    if parsed_speed.is_some() {
                        speed = parsed_speed;
                    }
                    if let Some(seconds) = fraction {
                        let _ = reader_tx.send(RenderEvent::Progress {
                            fraction: (seconds / duration_s).clamp(0.0, 1.0),
                            speed: speed.clone(),
                        });
                    }
                }
            }
        })
    });

    let outcome = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            break RenderEvent::Cancelled;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    RenderEvent::Done
                } else {
                    RenderEvent::Failed(format!(
                        "FFmpeg exited with {status}.{}",
                        log_tail(&log_path)
                    ))
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(80)),
            Err(e) => break RenderEvent::Failed(format!("Could not wait for FFmpeg: {e}")),
        }
    };

    if let Some(reader) = reader {
        let _ = reader.join();
    }

    let success = matches!(outcome, RenderEvent::Done) && plan.output.exists();
    if !success {
        let _ = std::fs::remove_file(&plan.output);
    }
    let _ = std::fs::remove_dir_all(&plan.workdir);
    outcome
}

fn log_tail(path: &Path) -> String {
    let mut text = String::new();
    if let Ok(mut file) = std::fs::File::open(path) {
        let _ = file.read_to_string(&mut text);
    }
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines
        .iter()
        .rev()
        .take(12)
        .rev()
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if tail.is_empty() {
        String::new()
    } else {
        format!("\n\n{tail}")
    }
}

/// Start a background render and return progress events.
pub fn start(plan: RenderPlan) -> RenderHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let output = plan.output.clone();
    spawn_worker(plan, tx, cancel.clone());
    RenderHandle {
        events: rx,
        cancel,
        output,
    }
}

/// Synchronous render used by the headless CLI.
pub fn render_blocking(
    plan: RenderPlan,
    mut on_progress: impl FnMut(f32, Option<&str>),
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    spawn_worker(plan, tx, cancel);
    for event in rx {
        match event {
            RenderEvent::Progress { fraction, speed } => {
                on_progress(fraction, speed.as_deref());
            }
            RenderEvent::Done => return Ok(()),
            RenderEvent::Failed(message) => return Err(message),
            RenderEvent::Cancelled => return Err("Render cancelled.".to_owned()),
        }
    }
    Err("Render ended unexpectedly.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::tokenize;

    fn project(container: Container, codec: AudioCodec) -> Project {
        let mut project = Project {
            title: "Winter Wrap Up".into(),
            instrumental: Some(PathBuf::from("winter_wrap_up.flac")),
            lines: tokenize("win/ter wrap up"),
            ..Project::default()
        };
        for i in 0..project.token_count() {
            project.set_token_start(i, 1_000 + i as u64 * 500);
        }
        project.export.container = container;
        project.export.audio_codec = codec;
        project.export.fps = 60;
        project
    }

    fn plan_for(_project: &Project) -> RenderPlan {
        RenderPlan {
            args: Vec::new(),
            workdir: PathBuf::from("."),
            duration_ms: 12_345,
            output: PathBuf::from("out.mp4"),
        }
    }

    #[test]
    fn builds_mp4_aac_arguments() {
        let project = project(Container::Mp4, AudioCodec::Aac);
        let plan = plan_for(&project);
        let args = build_args(&project, &plan, Path::new("background.png"));
        let joined = args.join(" ");
        assert!(joined.contains("-c:v libx264"));
        assert!(joined.contains("-crf 18"));
        assert!(joined.contains("-pix_fmt yuv420p"));
        assert!(joined.contains("-c:a aac"));
        assert!(joined.contains("-b:a 320k"));
        assert!(joined.contains("-movflags +faststart"));
        assert!(joined.contains("-t 12.345"));
        assert!(joined.contains("ass=karaoke.ass:fontsdir=fonts"));
        assert!(joined.contains("-map [v] -map 1:a"));
        assert!(joined.contains("winter_wrap_up.flac"));
        assert!(joined.ends_with("-f mp4 out.mp4"));
    }

    #[test]
    fn builds_classic_4_3_arguments() {
        let mut project = project(Container::Mp4, AudioCodec::Aac);
        project.export.aspect = crate::model::AspectRatio::Classic;
        let plan = plan_for(&project);
        let args = build_args(&project, &plan, Path::new("background.png"));
        let joined = args.join(" ");
        assert!(joined.contains("scale=1440:1080:force_original_aspect_ratio=increase"));
        assert!(joined.contains("crop=1440:1080"));
        assert!(joined.contains("-c:v libx264"));
    }

    #[test]
    fn builds_preroll_audio_delay_and_total_duration() {
        let mut project = project(Container::Mp4, AudioCodec::Aac);
        project.style.intro_preroll_secs = 3.5;
        let mut plan = plan_for(&project);
        plan.duration_ms = 8_000 + project.preroll_ms();
        let args = build_args(&project, &plan, Path::new("background.png"));
        let joined = args.join(" ");
        assert!(joined.contains("-af adelay=3500:all=1"));
        assert!(joined.contains("-t 11.500"));

        project.style.intro_preroll_secs = 0.0;
        let plan = plan_for(&project);
        let args = build_args(&project, &plan, Path::new("background.png"));
        assert!(!args.join(" ").contains("adelay"));
    }

    #[test]
    fn builds_mkv_flac_arguments() {
        let project = project(Container::Mkv, AudioCodec::Flac);
        let plan = plan_for(&project);
        let args = build_args(&project, &plan, Path::new("cover.jpg"));
        let joined = args.join(" ");
        assert!(joined.contains("-c:a flac"));
        assert!(!joined.contains("+faststart"));
        assert!(joined.ends_with("-f matroska out.mp4"));
        assert!(joined.contains("-loop 1 -framerate 60"));
    }

    #[test]
    fn parses_ffmpeg_progress_lines() {
        assert_eq!(
            parse_progress("out_time_us=5000000"),
            Some((Some(5.0), None))
        );
        assert_eq!(
            parse_progress("speed=1.25x"),
            Some((None, Some("1.25x".to_owned())))
        );
        assert_eq!(parse_progress("progress=end"), Some((Some(1.0e9), None)));
        assert_eq!(parse_progress("frame=42"), None);
    }

    #[test]
    fn gradient_is_written_and_readable() {
        let dir = util::temp_dir("karaokekit-test").unwrap();
        let path = dir.join("bg.png");
        write_gradient_png(&path, [0, 0, 0], [255, 255, 255], 1440, 1080).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.len() > 1_000);
        assert_eq!(image::image_dimensions(&path).unwrap(), (1440, 1080));
        let _ = std::fs::remove_dir_all(dir);
    }
}
