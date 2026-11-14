//! Scroll-free waveform overview: scrub, playhead, and lyric-line markers.

use crate::model::Project;
use crate::theme;
use crate::util::format_time;
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind,
    TextureHandle, Ui,
};

pub struct TimelineOutput {
    pub seek_ms: Option<u64>,
    pub response: Response,
}

pub fn show(
    ui: &mut Ui,
    project: &Project,
    waveform: Option<&TextureHandle>,
    position_ms: u64,
    duration_ms: u64,
    content_start_ms: u64,
    selected_line: Option<usize>,
) -> TimelineOutput {
    let height = 132.0;
    let width = ui.available_width().max(120.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect);

    painter.rect_filled(rect, CornerRadius::same(12), theme::CARD);
    painter.rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );

    let inner = rect.shrink2(egui::vec2(10.0, 18.0));
    let duration_ms = duration_ms.max(1);
    let to_x =
        |ms: u64| inner.left() + (ms.min(duration_ms) as f32 / duration_ms as f32) * inner.width();

    // The waveform represents the audio only; the title-card pre-roll is blank.
    let audio_start = content_start_ms.min(duration_ms);
    let wave_rect = Rect::from_min_max(Pos2::new(to_x(audio_start), inner.top()), inner.max);
    if let Some(texture) = waveform {
        if wave_rect.width() > 0.5 {
            painter.image(
                texture.id(),
                wave_rect,
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    } else {
        painter.text(
            wave_rect.center(),
            Align2::CENTER_CENTER,
            "Load an instrumental stem to see the waveform",
            FontId::proportional(13.0),
            theme::MUTED,
        );
    }
    if audio_start > 0 {
        let intro_rect =
            Rect::from_min_max(inner.min, Pos2::new(to_x(audio_start), inner.bottom()));
        painter.text(
            intro_rect.center(),
            Align2::CENTER_CENTER,
            "INTRO",
            FontId::monospace(9.0),
            theme::GOLD.gamma_multiply(0.8),
        );
        painter.line_segment(
            [
                Pos2::new(to_x(audio_start), inner.top()),
                Pos2::new(to_x(audio_start), inner.bottom()),
            ],
            Stroke::new(1.0, theme::GOLD.gamma_multiply(0.6)),
        );
    }

    let playhead_x = to_x(position_ms);

    // Dim the part that has not played yet.
    if playhead_x < inner.right() {
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(playhead_x, inner.top()), inner.max),
            CornerRadius::ZERO,
            Color32::from_black_alpha(85),
        );
    }
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(inner.left(), inner.top()),
            Pos2::new(playhead_x, inner.bottom()),
        ),
        CornerRadius::ZERO,
        Color32::from_rgba_unmultiplied(
            theme::ACCENT.r(),
            theme::ACCENT.g(),
            theme::ACCENT.b(),
            26,
        ),
    );

    // Lyric-line markers.
    let spans = project.spans();
    let mut flat = 0usize;
    for (line_index, line) in project.lines.iter().enumerate() {
        if let Some(start) = spans
            .iter()
            .skip(flat)
            .take(line.tokens.len())
            .flatten()
            .map(|(s, _)| *s)
            .min()
        {
            let x = to_x(start);
            let selected = selected_line == Some(line_index);
            painter.line_segment(
                [
                    Pos2::new(x, inner.bottom() - 15.0),
                    Pos2::new(x, inner.bottom()),
                ],
                Stroke::new(
                    if selected { 2.0 } else { 1.0 },
                    if selected {
                        theme::GOLD
                    } else {
                        theme::MUTED.gamma_multiply(0.7)
                    },
                ),
            );
        }
        flat += line.tokens.len();
    }

    // Ruler ticks.
    let step = if duration_ms > 300_000 {
        60_000
    } else if duration_ms > 90_000 {
        30_000
    } else {
        15_000
    };
    let mut tick = 0u64;
    while tick <= duration_ms {
        let x = to_x(tick);
        painter.line_segment(
            [Pos2::new(x, inner.top()), Pos2::new(x, inner.top() + 4.0)],
            Stroke::new(1.0, theme::BORDER),
        );
        if tick > 0 {
            painter.text(
                Pos2::new(x, inner.top() + 2.0),
                Align2::LEFT_TOP,
                format_time(tick),
                FontId::monospace(9.0),
                theme::MUTED,
            );
        }
        tick += step;
    }

    // Playhead.
    painter.line_segment(
        [
            Pos2::new(playhead_x, inner.top()),
            Pos2::new(playhead_x, inner.bottom()),
        ],
        Stroke::new(2.0, theme::GOLD),
    );
    let head = [
        Pos2::new(playhead_x - 6.0, inner.top()),
        Pos2::new(playhead_x + 6.0, inner.top()),
        Pos2::new(playhead_x, inner.top() + 8.0),
    ];
    painter.add(egui::Shape::convex_polygon(
        head.to_vec(),
        theme::GOLD,
        Stroke::NONE,
    ));

    // Hover line + time tooltip.
    let mut seek = None;
    if let Some(pointer) = response.interact_pointer_pos() {
        if rect.contains(pointer) {
            let fraction = ((pointer.x - inner.left()) / inner.width()).clamp(0.0, 1.0);
            let time = (fraction * duration_ms as f32) as u64;
            seek = Some(time);
            painter.line_segment(
                [
                    Pos2::new(pointer.x, inner.top()),
                    Pos2::new(pointer.x, inner.bottom()),
                ],
                Stroke::new(1.0, theme::TEXT.gamma_multiply(0.5)),
            );
            let label = format_time(time);
            let label_rect = Rect::from_min_size(
                Pos2::new((pointer.x + 8.0).min(rect.right() - 72.0), rect.top() + 4.0),
                egui::vec2(66.0, 18.0),
            );
            painter.rect_filled(
                label_rect,
                CornerRadius::same(4),
                Color32::from_black_alpha(170),
            );
            painter.text(
                label_rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::monospace(10.0),
                theme::TEXT,
            );
        }
    }
    if !response.dragged() && !response.clicked() {
        seek = None;
    }

    // Time labels.
    painter.text(
        Pos2::new(inner.left(), rect.bottom() - 16.0),
        Align2::LEFT_TOP,
        "00:00.00",
        FontId::monospace(9.0),
        theme::MUTED,
    );
    painter.text(
        Pos2::new(inner.right(), rect.bottom() - 16.0),
        Align2::RIGHT_TOP,
        format_time(duration_ms),
        FontId::monospace(9.0),
        theme::MUTED,
    );

    TimelineOutput {
        seek_ms: seek,
        response,
    }
}
