//! Live karaoke preview (16:9 or 4:3) with a per-syllable sweep.
//!
//! The stage is laid out against the same PlayRes coordinates as the generated
//! `.ass`, so the preview is a faithful WYSIWYG of the burned-in video (minus font
//! hinting differences).

use crate::model::{
    Background, Project, ON_DECK_LEAD_MS, PREROLL_ARTIST_SCALE, PREROLL_ARTIST_Y,
    PREROLL_TITLE_SCALE, PREROLL_TITLE_Y,
};
use crate::theme;
use egui::{
    text::{CCursor, LayoutJob, TextFormat},
    Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Response, Sense, Stroke,
    StrokeKind, TextureHandle, Ui, Vec2,
};
use std::sync::Arc;

const FAMILY: &str = "Karaoke";

struct LineInfo<'a> {
    line_index: usize,
    tokens: Vec<(usize, &'a crate::model::Token, u64, u64)>,
    start: u64,
    end: u64,
}

fn line_infos<'a>(project: &'a Project) -> Vec<LineInfo<'a>> {
    let spans = project.spans();
    let intervals = project.line_intervals();
    let mut infos = Vec::with_capacity(intervals.len());
    let mut flat = 0usize;
    for (line_index, line) in project.lines.iter().enumerate() {
        let Some(interval) = intervals
            .iter()
            .find(|interval| interval.index == line_index)
        else {
            flat += line.tokens.len();
            continue;
        };
        let mut tokens = Vec::new();
        for (index, token) in line.tokens.iter().enumerate() {
            if let Some((start, end)) = spans.get(flat + index).copied().flatten() {
                tokens.push((flat + index, token, start, end));
            }
        }
        flat += line.tokens.len();
        if !tokens.is_empty() {
            infos.push(LineInfo {
                line_index,
                tokens,
                start: interval.start,
                end: interval.end,
            });
        }
    }
    infos
}

fn fit_rect(available: Vec2, aspect: crate::model::AspectRatio) -> Vec2 {
    let width = available.x.max(64.0);
    let max_height = available.y.max(120.0);
    let mut height = width / aspect.ratio();
    let mut width = width;
    if height > max_height {
        height = max_height;
        width = height * aspect.ratio();
    }
    egui::vec2(width, height)
}

fn cover_uv(texture: &TextureHandle, rect: Rect) -> Rect {
    let tex = texture.size_vec2();
    if tex.x <= 0.0 || tex.y <= 0.0 {
        return Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    }
    let src_aspect = tex.x / tex.y;
    let dst_aspect = rect.width() / rect.height().max(1.0);
    if src_aspect > dst_aspect {
        let visible = (dst_aspect / src_aspect).clamp(0.01, 1.0);
        let pad = (1.0 - visible) * 0.5;
        Rect::from_min_max(Pos2::new(pad, 0.0), Pos2::new(1.0 - pad, 1.0))
    } else {
        let visible = (src_aspect / dst_aspect).clamp(0.01, 1.0);
        let pad = (1.0 - visible) * 0.5;
        Rect::from_min_max(Pos2::new(0.0, pad), Pos2::new(1.0, 1.0 - pad))
    }
}

fn galley_for(
    ui: &Ui,
    text: &str,
    font_id: FontId,
    color: Color32,
    wrap: f32,
) -> Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    job.append(
        text,
        0.0,
        TextFormat {
            // Libass line spacing is tighter than egui's default. Matching it
            // keeps the preview block height in sync with the exported video.
            line_height: Some(font_id.size * 1.05),
            font_id,
            color,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

fn gradient_mesh(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

fn text_height(galley: &egui::Galley) -> f32 {
    galley.size().y
}

/// Draw the preview and return its response.
pub fn show(
    ui: &mut Ui,
    project: &Project,
    position_ms: u64,
    cover: Option<&TextureHandle>,
    background_thumb: Option<&TextureHandle>,
) -> Response {
    let available = ui.available_size();
    let size = fit_rect(available, project.export.aspect);
    // Allocate a full-width row so a 4:3 stage can be centered inside the workspace.
    let (row_rect, response) =
        ui.allocate_exact_size(egui::vec2(available.x.max(size.x), size.y), Sense::hover());
    let rect = Rect::from_center_size(row_rect.center(), size);
    let painter = ui.painter().with_clip_rect(rect);
    let style = &project.style;

    painter.rect_filled(rect, CornerRadius::same(12), theme::BG);
    let inner = rect.shrink(1.0);
    match &style.background {
        Background::Gradient { top, bottom } => {
            gradient_mesh(
                &painter,
                inner,
                Color32::from_rgb(top[0], top[1], top[2]),
                Color32::from_rgb(bottom[0], bottom[1], bottom[2]),
            );
        }
        Background::Cover { dim, .. } => {
            if let Some(texture) = cover {
                painter.image(
                    texture.id(),
                    inner,
                    cover_uv(texture, inner),
                    Color32::WHITE,
                );
                painter.rect_filled(
                    inner,
                    CornerRadius::ZERO,
                    Color32::from_black_alpha((dim.clamp(0.0, 0.9) * 255.0) as u8),
                );
            } else {
                gradient_mesh(&painter, inner, theme::CARD, theme::BG);
                painter.text(
                    inner.center(),
                    Align2::CENTER_CENTER,
                    "Choose a cover image",
                    FontId::proportional(16.0),
                    theme::MUTED,
                );
            }
        }
        Background::Video { dim, .. } => {
            if let Some(texture) = background_thumb {
                painter.image(
                    texture.id(),
                    inner,
                    cover_uv(texture, inner),
                    Color32::WHITE,
                );
            } else {
                gradient_mesh(&painter, inner, theme::CARD, theme::BG);
            }
            painter.rect_filled(
                inner,
                CornerRadius::ZERO,
                Color32::from_black_alpha((dim.clamp(0.0, 0.9) * 255.0) as u8),
            );
        }
    }

    // Subtle vignette for text legibility.
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(inner.left(), inner.bottom() - inner.height() * 0.34),
            inner.max,
        ),
        CornerRadius::ZERO,
        Color32::from_black_alpha(60),
    );

    let scale = rect.height() / 1080.0;
    let font_size = (style.font_size * scale).clamp(10.0, 220.0);
    let font_id = FontId::new(font_size, FontFamily::Name(Arc::from(FAMILY)));
    let margin = (style.margin_v * scale).clamp(0.0, rect.height() * 0.5);
    let wrap = rect.width() * 0.86;
    let base = Color32::from_rgb(
        style.base_color[0],
        style.base_color[1],
        style.base_color[2],
    );
    let highlight = Color32::from_rgb(
        style.highlight_color[0],
        style.highlight_color[1],
        style.highlight_color[2],
    );
    let next_rgb = style.next_line_color();
    let next_color = Color32::from_rgb(next_rgb[0], next_rgb[1], next_rgb[2]);
    let outline = (style.outline_width * scale).clamp(0.0, 20.0);
    let shadow = (style.shadow_width * scale).clamp(0.0, 20.0);
    let inner_text = rect.shrink2(egui::vec2(rect.width() * 0.07, 0.0));

    let infos = line_infos(project);
    if infos.is_empty() {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Paste lyrics, then tap Space along with the song",
            FontId::proportional(16.0),
            theme::MUTED,
        );
        return response;
    }

    // Strict active interval: start <= t <= end. The upcoming line is shown
    // above only when it is within the on-deck lookahead; during a gap, the
    // preview shows just the on-deck line (or nothing).
    let current = project
        .active_line(position_ms)
        .and_then(|line| infos.iter().position(|info| info.line_index == line));
    let next_above = current.and_then(|index| {
        infos
            .get(index + 1)
            .filter(|info| infos[index].end.saturating_add(ON_DECK_LEAD_MS) >= info.start)
    });
    let on_deck = if current.is_none() {
        project
            .on_deck_line(position_ms, ON_DECK_LEAD_MS)
            .and_then(|line| infos.iter().position(|info| info.line_index == line))
    } else {
        None
    };
    let countdown = project.countdown_cue(position_ms);

    let draw_line = |info: &LineInfo<'_>, color: Color32, sweep: bool, y_bottom: f32| -> f32 {
        if info.tokens.is_empty() {
            return 0.0;
        }
        let mut text = String::new();
        let mut ranges: Vec<(usize, usize, usize)> = Vec::new(); // flat, char start, char end
        let mut char_count = 0usize;
        for (i, (flat, token, _, _)) in info.tokens.iter().enumerate() {
            if i > 0 && token.word_start {
                text.push(' ');
                char_count += 1;
            }
            let start_char = char_count;
            text.push_str(&token.text);
            char_count += token.text.chars().count();
            ranges.push((*flat, start_char, char_count));
        }

        let galley = galley_for(ui, &text, font_id.clone(), color, wrap);
        let pos = Pos2::new(
            rect.center().x - galley.size().x * 0.5,
            y_bottom - text_height(&galley),
        );

        // Outline + drop shadow (cheap faux stroke using offset copies).
        if outline > 0.5 || shadow > 0.5 {
            let steps = if outline > 0.5 { 8 } else { 0 };
            for step in 0..steps {
                let angle = std::f32::consts::TAU * step as f32 / steps as f32;
                let offset = egui::vec2(angle.cos(), angle.sin()) * outline;
                painter.galley_with_override_text_color(
                    pos + offset,
                    galley.clone(),
                    Color32::from_black_alpha(200),
                );
            }
            if shadow > 0.5 {
                painter.galley_with_override_text_color(
                    pos + egui::vec2(shadow * 0.85, shadow),
                    galley.clone(),
                    Color32::from_black_alpha(150),
                );
            }
        }
        painter.galley(pos, galley.clone(), color);

        if sweep {
            let highlight_galley = galley_for(ui, &text, font_id.clone(), highlight, wrap);
            for (flat, char_start, char_end) in ranges {
                let Some((start, end)) = project.token_span(flat) else {
                    continue;
                };
                let progress = if position_ms >= end {
                    1.0
                } else if position_ms <= start {
                    0.0
                } else {
                    (position_ms - start) as f32 / (end - start).max(1) as f32
                };
                if progress <= 0.0 {
                    continue;
                }
                let x0 = painter.pos_from_cursor(galley.as_ref(), pos, CCursor::new(char_start));
                let mut x1 = painter.pos_from_cursor(galley.as_ref(), pos, CCursor::new(char_end));
                if progress < 1.0 {
                    x1 = x0 + (x1 - x0) * progress.clamp(0.0, 1.0);
                }
                let clip = Rect::from_min_max(
                    Pos2::new(x0, pos.y - outer_padding(font_size)),
                    Pos2::new(
                        x1.max(x0 + 0.5),
                        pos.y + galley.size().y + outer_padding(font_size),
                    ),
                )
                .intersect(inner_text);
                if clip.width() > 0.0 {
                    painter
                        .with_clip_rect(clip)
                        .galley(pos, highlight_galley.clone(), highlight);
                }
            }
        }

        galley.size().y
    };

    let bottom_y = rect.bottom() - margin;
    let gap = font_size * 0.12;

    // Title-card pre-roll: title/artist, the opening line on deck, and the
    // 4-dot countdown to the audio start.
    let prereoll_ms = project.preroll_ms();
    if prereoll_ms > 0 && position_ms < prereoll_ms {
        let family = FontFamily::Name(Arc::from(FAMILY));
        let title_font = FontId::new(
            (font_size * PREROLL_TITLE_SCALE).clamp(12.0, 300.0),
            family.clone(),
        );
        let artist_font = FontId::new((font_size * PREROLL_ARTIST_SCALE).clamp(8.0, 200.0), family);
        let title_y = rect.top() + rect.height() * PREROLL_TITLE_Y;
        let artist_y = rect.top() + rect.height() * PREROLL_ARTIST_Y;
        draw_centered_text(
            ui,
            &painter,
            &project.title,
            title_font,
            highlight,
            rect.center().x,
            title_y,
            wrap,
            outline,
            shadow,
        );
        draw_centered_text(
            ui,
            &painter,
            &project.artist,
            artist_font,
            next_color,
            rect.center().x,
            artist_y,
            wrap,
            outline,
            shadow,
        );
        if let Some(first) = infos.first() {
            let deck_height = draw_line(first, next_color, false, bottom_y);
            if let Some(dots) = project.preroll_dots(position_ms) {
                draw_countdown_dots(
                    ui,
                    &painter,
                    rect,
                    font_size,
                    bottom_y - deck_height,
                    dots,
                    highlight,
                );
            }
        }
        painter.rect_stroke(
            rect,
            CornerRadius::same(12),
            Stroke::new(1.0, theme::BORDER),
            StrokeKind::Inside,
        );
        return response;
    }

    if let Some(index) = current {
        let current_height = draw_line(&infos[index], base, true, bottom_y);
        if let Some(next) = next_above {
            // Stack above the current line using its real rendered height.
            draw_line(next, next_color, false, bottom_y - current_height - gap);
        }
    } else if let Some(index) = on_deck {
        let deck_height = draw_line(&infos[index], next_color, false, bottom_y);
        if let Some(cue) = countdown {
            if cue.line == infos[index].line_index {
                draw_countdown_dots(
                    ui,
                    &painter,
                    rect,
                    font_size,
                    bottom_y - deck_height,
                    cue.dots,
                    highlight,
                );
            }
        }
    }

    painter.rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );

    response
}

#[allow(clippy::too_many_arguments)]
fn draw_centered_text(
    ui: &Ui,
    painter: &egui::Painter,
    text: &str,
    font_id: FontId,
    color: Color32,
    center_x: f32,
    center_y: f32,
    wrap: f32,
    outline: f32,
    shadow: f32,
) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let galley = galley_for(ui, text, font_id, color, wrap);
    let pos = Pos2::new(
        center_x - galley.size().x * 0.5,
        center_y - galley.size().y * 0.5,
    );
    if outline > 0.5 || shadow > 0.5 {
        let steps = if outline > 0.5 { 8 } else { 0 };
        for step in 0..steps {
            let angle = std::f32::consts::TAU * step as f32 / steps as f32;
            let offset = egui::vec2(angle.cos(), angle.sin()) * outline;
            painter.galley_with_override_text_color(
                pos + offset,
                galley.clone(),
                Color32::from_black_alpha(200),
            );
        }
        if shadow > 0.5 {
            painter.galley_with_override_text_color(
                pos + egui::vec2(shadow * 0.85, shadow),
                galley.clone(),
                Color32::from_black_alpha(150),
            );
        }
    }
    painter.galley(pos, galley, color);
}

fn draw_countdown_dots(
    ui: &Ui,
    painter: &egui::Painter,
    canvas: Rect,
    lyric_font_size: f32,
    deck_top: f32,
    dots: usize,
    color: Color32,
) {
    if dots == 0 {
        return;
    }
    let text = crate::model::countdown_text(dots);
    let font_size =
        (lyric_font_size * crate::model::COUNTDOWN_DOT_SCALE).clamp(8.0, lyric_font_size);
    let font_id = FontId::new(font_size, FontFamily::Name(Arc::from(FAMILY)));
    let galley = galley_for(ui, &text, font_id, color, canvas.width() * 0.8);
    let gap = lyric_font_size * 0.12;
    let pos = Pos2::new(
        canvas.center().x - galley.size().x * 0.5,
        deck_top - gap - galley.size().y,
    );
    painter.galley(pos, galley, color);
}

fn outer_padding(font_size: f32) -> f32 {
    font_size * 0.35
}

trait CursorPos {
    fn pos_from_cursor(&self, galley: &egui::Galley, origin: Pos2, cursor: CCursor) -> f32;
}

impl CursorPos for egui::Painter {
    fn pos_from_cursor(&self, galley: &egui::Galley, origin: Pos2, cursor: CCursor) -> f32 {
        origin.x + galley.pos_from_cursor(cursor).left()
    }
}
