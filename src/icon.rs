//! Window icon loading plus a generated fallback, so the app always has a clean
//! icon even when `assets/icon.png` was stripped from a distribution.

use egui::IconData;
use image::{DynamicImage, ImageFormat, RgbaImage};
use std::io::Cursor;
use std::path::PathBuf;

pub const ICON_SIZE: u32 = 512;

/// Try `assets/icon.png` on disk, then the embedded copy, then draw a fallback.
pub fn load_icon() -> IconData {
    for candidate in icon_candidates() {
        if let Ok(bytes) = std::fs::read(&candidate) {
            if let Some(icon) = decode_icon(&bytes) {
                return icon;
            }
        }
    }
    if let Some(icon) = decode_icon(crate::assets::ICON_PNG) {
        return icon;
    }
    generated_icon()
}

fn icon_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("assets").join("icon.png"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("assets").join("icon.png"));
            candidates.push(dir.join("icon.png"));
        }
    }
    candidates
}

pub fn decode_icon(bytes: &[u8]) -> Option<IconData> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    Some(IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

pub fn generated_icon() -> IconData {
    generated_icon_sized(ICON_SIZE)
}

pub fn generated_icon_sized(size: u32) -> IconData {
    IconData {
        rgba: generate_rgba(size),
        width: size,
        height: size,
    }
}

/// Render the fallback icon (rounded gradient tile, eighth note, sweep bar).
/// Drawn at 2× and box-filtered down for clean edges.
pub fn generate_rgba(size: u32) -> Vec<u8> {
    let size = size.max(8);
    let ss = 2u32;
    let high = size * ss;
    let mut hi = vec![[0.0f32; 4]; (high * high) as usize];
    for y in 0..high {
        for x in 0..high {
            let u = (x as f32 + 0.5) / high as f32;
            let v = (y as f32 + 0.5) / high as f32;
            hi[(y * high + x) as usize] = shade(u, v);
        }
    }

    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let px = x * ss + sx;
                    let py = y * ss + sy;
                    let c = hi[(py * high + px) as usize];
                    for k in 0..4 {
                        acc[k] += c[k];
                    }
                }
            }
            let n = (ss * ss) as f32;
            let i = ((y * size + x) * 4) as usize;
            out[i] = (acc[0] / n * 255.0).round().clamp(0.0, 255.0) as u8;
            out[i + 1] = (acc[1] / n * 255.0).round().clamp(0.0, 255.0) as u8;
            out[i + 2] = (acc[2] / n * 255.0).round().clamp(0.0, 255.0) as u8;
            out[i + 3] = (acc[3] / n * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn shade(x: f32, y: f32) -> [f32; 4] {
    // Rounded tile.
    if !inside_rounded(x, y, 0.5, 0.5, 0.47, 0.47, 0.22) {
        return [0.0, 0.0, 0.0, 0.0];
    }
    let t = ((x + y) * 0.5).clamp(0.0, 1.0);
    let top = [
        0x6C as f32 / 255.0,
        0x5C as f32 / 255.0,
        0xE7 as f32 / 255.0,
    ];
    let bottom = [
        0xFD as f32 / 255.0,
        0x79 as f32 / 255.0,
        0xA8 as f32 / 255.0,
    ];
    let mut color = [
        top[0] + (bottom[0] - top[0]) * t,
        top[1] + (bottom[1] - top[1]) * t,
        top[2] + (bottom[2] - top[2]) * t,
        1.0,
    ];
    // Soft sheen towards the top-left.
    let sheen = (1.0 - ((x - 0.25).powi(2) + (y - 0.2).powi(2)).sqrt() * 1.6).max(0.0) * 0.18;
    for c in color.iter_mut().take(3) {
        *c = (*c + sheen).min(1.0);
    }

    let white = [0.99f32, 0.99, 1.0, 1.0];
    // Note head.
    if inside_ellipse(x, y, 0.365, 0.675, 0.155, 0.104, -0.35) {
        color = white;
    }
    // Stem.
    if (0.495..=0.555).contains(&x) && (0.285..=0.68).contains(&y) {
        color = white;
    }
    // Flag.
    if point_in_polygon(
        x,
        y,
        &[(0.53, 0.285), (0.725, 0.37), (0.695, 0.475), (0.53, 0.415)],
    ) {
        color = white;
    }
    // Sweep bar: sung part gold, remaining part translucent white.
    if inside_rounded(x, y, 0.5, 0.855, 0.33, 0.023, 0.023) {
        let gold = [
            0xFF as f32 / 255.0,
            0xD5 as f32 / 255.0,
            0x4A as f32 / 255.0,
        ];
        if x < 0.5 - 0.33 + 0.33 * 2.0 * 0.62 {
            color = [gold[0], gold[1], gold[2], 1.0];
        } else {
            color = [1.0, 1.0, 1.0, 0.38];
        }
    }
    color
}

fn inside_rounded(x: f32, y: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> bool {
    let dx = (x - cx).abs() - (hw - r);
    let dy = (y - cy).abs() - (hh - r);
    let ox = dx.max(0.0);
    let oy = dy.max(0.0);
    ox * ox + oy * oy <= r * r && (x - cx).abs() <= hw && (y - cy).abs() <= hh
}

fn inside_ellipse(x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32, angle: f32) -> bool {
    let (dx, dy) = (x - cx, y - cy);
    let (s, c) = angle.sin_cos();
    let px = dx * c - dy * s;
    let py = dx * s + dy * c;
    (px / rx).powi(2) + (py / ry).powi(2) <= 1.0
}

fn point_in_polygon(x: f32, y: f32, points: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Encode the generated icon as PNG (used by the `gen_icon` example).
pub fn png_bytes(size: u32) -> Result<Vec<u8>, String> {
    let rgba = generate_rgba(size);
    let image = RgbaImage::from_raw(size, size, rgba)
        .ok_or_else(|| "Could not build icon buffer.".to_owned())?;
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, ImageFormat::Png)
        .map_err(|e| format!("Could not encode icon PNG: {e}"))?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_icon_has_expected_shape() {
        let rgba = generate_rgba(128);
        assert_eq!(rgba.len(), 128 * 128 * 4);
        let center = ((64 * 128 + 64) * 4) as usize;
        assert!(rgba[center + 3] > 200, "centre should be opaque");
        assert_eq!(rgba[3], 0, "rounded corner should be transparent");
    }

    #[test]
    fn generates_valid_png() {
        let bytes = png_bytes(96).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        let icon = decode_icon(&bytes).unwrap();
        assert_eq!((icon.width, icon.height), (96, 96));
    }
}
