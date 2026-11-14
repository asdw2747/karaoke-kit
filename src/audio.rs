//! Stem decoding (Symphonia), waveform extraction, and sample-accurate playback (CPAL).
//!
//! Both stems are decoded once into interleaved i16 stereo. A single always-running
//! output stream mixes them with independent gains/mutes. The UI playhead is derived
//! from the callback frame counter, extrapolated with the callback clock, and shifted
//! by the reported output latency so taps land on the audio the user actually hears.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub const PEAK_BINS: usize = 4096;
const NO_SEEK: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Peak {
    pub min: f32,
    pub max: f32,
}

#[derive(Debug)]
pub struct Track {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub samples: Vec<i16>, // interleaved L/R
    pub duration_ms: u64,
    pub peaks: Vec<Peak>,
}

impl Track {
    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    pub fn label(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Decode any supported stem format to interleaved i16 stereo.
pub fn decode_track(path: &Path) -> Result<Track, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(&ext.to_ascii_lowercase());
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("{}: unsupported audio format ({e})", path.display()))?;
    let mut format = probed.format;

    let track = format
        .default_track()
        .or_else(|| format.tracks().first())
        .ok_or_else(|| format!("{}: no audio track found", path.display()))?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let sample_rate = params
        .sample_rate
        .filter(|r| *r > 0)
        .ok_or_else(|| format!("{}: unknown sample rate", path.display()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| format!("{}: unsupported codec ({e})", path.display()))?;

    let mut samples: Vec<i16> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(format!("{}: read error ({e})", path.display())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let ch = spec.channels.count().max(1);
                let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                buffer.copy_interleaved_ref(decoded);
                for frame in buffer.samples().chunks(ch) {
                    let left = frame[0];
                    let right = if ch > 1 { frame[1] } else { frame[0] };
                    samples.push((left.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
                    samples.push((right.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
                }
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(format!("{}: decode error ({e})", path.display())),
        }
    }

    if samples.is_empty() {
        return Err(format!("{}: no audio samples decoded", path.display()));
    }
    let frames = samples.len() / 2;
    let duration_ms = (frames as u64 * 1000) / sample_rate as u64;
    let peaks = compute_peaks(&samples, PEAK_BINS);
    Ok(Track {
        path: path.to_owned(),
        sample_rate,
        samples,
        duration_ms,
        peaks,
    })
}

/// Read the duration without decoding the whole file when the container
/// exposes a frame count (FLAC/WAV do; MP3 falls back to a full decode).
pub fn probe_duration(path: &Path) -> Result<u64, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(&ext.to_ascii_lowercase());
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("{}: unsupported audio format ({e})", path.display()))?;
    if let Some(track) = probed.format.default_track() {
        let params = &track.codec_params;
        if let (Some(frames), Some(rate)) = (params.n_frames, params.sample_rate) {
            if frames > 0 && rate > 0 {
                return Ok(frames * 1000 / rate as u64);
            }
        }
    }
    decode_track(path).map(|track| track.duration_ms)
}

/// Min/max envelope per bin, normalised for display.
pub fn compute_peaks(samples: &[i16], bins: usize) -> Vec<Peak> {
    let frames = samples.len() / 2;
    let bins = bins.max(1);
    if frames == 0 {
        return vec![Peak::default(); bins];
    }
    let mut peaks = Vec::with_capacity(bins);
    for bin in 0..bins {
        let start = bin * frames / bins;
        let end = ((bin + 1) * frames / bins).max(start + 1).min(frames);
        let mut min = 0.0f32;
        let mut max = 0.0f32;
        for frame in start..end {
            let l = samples[frame * 2] as f32 / i16::MAX as f32;
            let r = samples[frame * 2 + 1] as f32 / i16::MAX as f32;
            let value = (l + r) * 0.5;
            min = min.min(value);
            max = max.max(value);
        }
        peaks.push(Peak { min, max });
    }
    let loudest = peaks
        .iter()
        .fold(0.0f32, |acc, p| acc.max(p.min.abs()).max(p.max.abs()));
    if loudest > 0.0001 {
        let scale = 0.95 / loudest;
        for peak in &mut peaks {
            peak.min *= scale;
            peak.max *= scale;
        }
    }
    peaks
}

/// Linear-interpolated stereo sample at a source time (seconds).
fn sample_at(track: &Track, seconds: f64) -> (f32, f32) {
    let frames = track.frames();
    if frames == 0 || !seconds.is_finite() || seconds < 0.0 {
        return (0.0, 0.0);
    }
    let frame = seconds * track.sample_rate as f64;
    if frame >= frames as f64 {
        return (0.0, 0.0);
    }
    let i0 = frame.floor() as usize;
    let frac = (frame - i0 as f64) as f32;
    let i1 = (i0 + 1).min(frames.saturating_sub(1));
    let l0 = track.samples[i0 * 2] as f32 / i16::MAX as f32;
    let r0 = track.samples[i0 * 2 + 1] as f32 / i16::MAX as f32;
    let l1 = track.samples[i1 * 2] as f32 / i16::MAX as f32;
    let r1 = track.samples[i1 * 2 + 1] as f32 / i16::MAX as f32;
    (l0 + (l1 - l0) * frac, r0 + (r1 - r0) * frac)
}

struct Shared {
    instrumental: Arc<Track>,
    vocal: Option<Arc<Track>>,
    device_rate: u32,
    preroll_ms: u64,
    total_ms: u64,
    position: AtomicU64,
    /// Position at the start of the last callback (used for UI extrapolation).
    ui_position: AtomicU64,
    seek: AtomicU64,
    playing: AtomicBool,
    inst_gain: AtomicU32,
    vocal_gain: AtomicU32,
    inst_mute: AtomicBool,
    vocal_mute: AtomicBool,
    started: Instant,
    callback_micros: AtomicU64,
    latency_micros: AtomicU64,
}

pub struct AudioEngine {
    shared: Arc<Shared>,
    _stream: Stream,
}

fn gain_bits(gain: f32) -> u32 {
    gain.clamp(0.0, 2.0).to_bits()
}

fn bits_gain(bits: u32) -> f32 {
    f32::from_bits(bits)
}

impl AudioEngine {
    /// Build an output stream and mix `instrumental` (required) with `vocal` (optional).
    pub fn new(
        instrumental: Arc<Track>,
        vocal: Option<Arc<Track>>,
        start_ms: u64,
        preroll_ms: u64,
    ) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "No audio output device was found.".to_owned())?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("Could not query the audio device: {e}"))?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.config();
        let device_rate = config.sample_rate.max(1);
        let total_ms = preroll_ms.saturating_add(instrumental.duration_ms);

        let start = start_ms.min(total_ms);
        let shared = Arc::new(Shared {
            instrumental,
            vocal,
            device_rate,
            preroll_ms,
            total_ms,
            position: AtomicU64::new(start * device_rate as u64 / 1000),
            ui_position: AtomicU64::new(start * device_rate as u64 / 1000),
            seek: AtomicU64::new(NO_SEEK),
            playing: AtomicBool::new(false),
            inst_gain: AtomicU32::new(gain_bits(1.0)),
            vocal_gain: AtomicU32::new(gain_bits(0.8)),
            inst_mute: AtomicBool::new(false),
            vocal_mute: AtomicBool::new(false),
            started: Instant::now(),
            callback_micros: AtomicU64::new(0),
            latency_micros: AtomicU64::new(0),
        });

        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, shared.clone()),
            SampleFormat::I16 => build_stream::<i16>(&device, &config, shared.clone()),
            SampleFormat::U16 => build_stream::<u16>(&device, &config, shared.clone()),
            other => Err(format!(
                "The default audio device uses an unsupported sample format ({other:?})."
            )),
        }?;

        stream
            .play()
            .map_err(|e| format!("Could not start audio playback: {e}"))?;
        Ok(Self {
            shared,
            _stream: stream,
        })
    }

    pub fn play(&self) {
        if self.position_ms() + 15 >= self.shared.total_ms {
            self.seek(0);
        }
        self.shared.playing.store(true, Ordering::Relaxed);
    }

    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn toggle(&self) {
        if self.is_playing() {
            self.pause();
        } else {
            self.play();
        }
    }

    pub fn seek(&self, ms: u64) {
        let ms = ms.min(self.shared.total_ms);
        let frames = ms * self.shared.device_rate as u64 / 1000;
        self.shared.seek.store(frames, Ordering::Relaxed);
        self.shared.position.store(frames, Ordering::Relaxed);
        self.shared.ui_position.store(frames, Ordering::Relaxed);
        self.shared.callback_micros.store(
            self.shared.started.elapsed().as_micros() as u64,
            Ordering::Relaxed,
        );
    }

    pub fn position_ms(&self) -> u64 {
        let playing = self.shared.playing.load(Ordering::Relaxed);
        let frames = if playing {
            self.shared.ui_position.load(Ordering::Relaxed)
        } else {
            self.shared.position.load(Ordering::Relaxed)
        };
        let base = frames * 1000 / self.shared.device_rate as u64;
        if !playing {
            return base.min(self.shared.total_ms);
        }
        let now = self.shared.started.elapsed().as_micros() as u64;
        let callback = self.shared.callback_micros.load(Ordering::Relaxed);
        let elapsed_ms = now.saturating_sub(callback).min(50_000) / 1000;
        let latency_ms = self.shared.latency_micros.load(Ordering::Relaxed) / 1000;
        (base + elapsed_ms)
            .saturating_sub(latency_ms)
            .min(self.shared.total_ms)
    }

    pub fn duration_ms(&self) -> u64 {
        self.shared.total_ms
    }

    pub fn preroll_ms(&self) -> u64 {
        self.shared.preroll_ms
    }

    pub fn set_gains(&self, instrumental: f32, vocal: f32) {
        self.shared
            .inst_gain
            .store(gain_bits(instrumental), Ordering::Relaxed);
        self.shared
            .vocal_gain
            .store(gain_bits(vocal), Ordering::Relaxed);
    }

    pub fn set_mutes(&self, instrumental: bool, vocal: bool) {
        self.shared.inst_mute.store(instrumental, Ordering::Relaxed);
        self.shared.vocal_mute.store(vocal, Ordering::Relaxed);
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    shared: Arc<Shared>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels.max(1) as usize;
    let rate = config.sample_rate as f32;
    let fade_step = 1.0 / (rate * 0.010).max(1.0);
    let mut ramp = 0.0f32;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                render_audio(data, channels, &shared, &mut ramp, fade_step, info);
            },
            move |err| eprintln!("KaraokeKit audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("Could not open the audio output stream: {e}"))
}

fn render_audio<T>(
    data: &mut [T],
    channels: usize,
    shared: &Shared,
    ramp: &mut f32,
    fade_step: f32,
    info: &cpal::OutputCallbackInfo,
) where
    T: SizedSample + FromSample<f32>,
{
    let callback_started = shared.started.elapsed().as_micros() as u64;
    let seek = shared.seek.swap(NO_SEEK, Ordering::Relaxed);
    let mut position = shared.position.load(Ordering::Relaxed);
    if seek != NO_SEEK {
        position = seek;
        *ramp = 0.0;
    }
    let start_position = position;

    let mut playing = shared.playing.load(Ordering::Relaxed);
    let mut reached_end = false;
    let inst_gain = if shared.inst_mute.load(Ordering::Relaxed) {
        0.0
    } else {
        bits_gain(shared.inst_gain.load(Ordering::Relaxed))
    };
    let vocal_gain = if shared.vocal_mute.load(Ordering::Relaxed) {
        0.0
    } else {
        bits_gain(shared.vocal_gain.load(Ordering::Relaxed))
    };
    let rate = shared.device_rate as f64;
    let prereoll_seconds = shared.preroll_ms as f64 / 1000.0;
    let max_frames = shared.total_ms * shared.device_rate as u64 / 1000;

    for frame in data.chunks_mut(channels) {
        let target = if playing { 1.0 } else { 0.0 };
        *ramp = if *ramp < target {
            (*ramp + fade_step).min(target)
        } else {
            (*ramp - fade_step).max(target)
        };
        let active = playing || *ramp > 0.0001;

        let (mut left, mut right) = (0.0f32, 0.0f32);
        if active {
            let seconds = position as f64 / rate - prereoll_seconds;
            let (l, r) = sample_at(&shared.instrumental, seconds);
            left += l * inst_gain * *ramp;
            right += r * inst_gain * *ramp;
            if let Some(vocal) = &shared.vocal {
                let (vl, vr) = sample_at(vocal, seconds);
                left += vl * vocal_gain * *ramp;
                right += vr * vocal_gain * *ramp;
            }
            left = left.clamp(-1.0, 1.0);
            right = right.clamp(-1.0, 1.0);

            if position + 1 >= max_frames {
                playing = false;
                reached_end = true;
                position = max_frames;
            } else {
                position += 1;
            }
        }

        if channels == 1 {
            let value: T = T::from_sample((left + right) * 0.5);
            frame[0] = value;
        } else {
            let l: T = T::from_sample(left);
            let r: T = T::from_sample(right);
            frame[0] = l;
            if channels > 1 {
                frame[1] = r;
            }
            for sample in frame.iter_mut().skip(2) {
                *sample = T::from_sample(0.0);
            }
        }
    }

    if reached_end || !playing {
        shared.playing.store(false, Ordering::Relaxed);
    }
    // Persist the advanced position for the next callback, and publish the
    // callback-start position separately so the UI can extrapolate smoothly.
    shared.position.store(position, Ordering::Relaxed);
    shared.ui_position.store(
        if playing { start_position } else { position },
        Ordering::Relaxed,
    );
    shared
        .callback_micros
        .store(callback_started, Ordering::Relaxed);
    let timestamp = info.timestamp();
    if let Some(latency) = timestamp.playback.duration_since(&timestamp.callback) {
        shared
            .latency_micros
            .store(latency.as_micros() as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_track_waveform_shape() {
        let mut samples = Vec::new();
        for i in 0..1_000 {
            let v = if i < 500 { 0.0 } else { 0.5 };
            let s = (v * i16::MAX as f32) as i16;
            samples.push(s);
            samples.push(s);
        }
        let peaks = compute_peaks(&samples, 100);
        assert_eq!(peaks.len(), 100);
        assert!(peaks[10].max.abs() < 0.01);
        assert!(peaks[90].max > 0.4);
    }

    #[test]
    fn sample_interpolation_is_safe_at_edges() {
        let track = Track {
            path: PathBuf::from("test.flac"),
            sample_rate: 2,
            samples: vec![0, 0, i16::MAX, i16::MAX],
            duration_ms: 1_000,
            peaks: Vec::new(),
        };
        assert_eq!(sample_at(&track, -1.0), (0.0, 0.0));
        assert_eq!(sample_at(&track, 100.0), (0.0, 0.0));
        let (l, _) = sample_at(&track, 0.25);
        assert!(l > 0.4 && l < 0.6);
    }
}
