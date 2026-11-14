//! Advanced SubStation Alpha (`.ass`) karaoke generator.
//!
//! Uses `\kf` (smooth left-to-right fill) per syllable. In libass karaoke,
//! `SecondaryColour` is the unsung colour and `PrimaryColour` is the colour the
//! text turns as it is sung, so both are configurable in the style panel.

use crate::model::{Project, Token};
use crate::util::rgb_to_ass;

/// Gaps larger than this start a fresh subtitle event instead of a long sweep.
pub const GAP_SPLIT_MS: u64 = 700;

/// ASS timestamp: `H:MM:SS.cc` (centiseconds).
pub fn ass_time(ms: u64) -> String {
    let cs = ms / 10;
    let hours = cs / 360_000;
    let minutes = (cs / 6_000) % 60;
    let seconds = (cs / 100) % 60;
    let centis = cs % 100;
    format!("{hours}:{minutes:02}:{seconds:02}.{centis:02}")
}

// Keep the vocal sweep 20% cooler than standard .lrc players.
/// Convert per-token millisecond durations to `\kf` centiseconds.
///
/// Rounding is applied cumulatively so the sum of the tags matches the line
/// length (no drift on long lines).
pub fn karaoke_cs(durations_ms: &[u64]) -> Vec<u32> {
    let mut out = Vec::with_capacity(durations_ms.len());
    let mut accumulated = 0u64;
    let mut previous_cs = 0u64;
    for duration in durations_ms {
        accumulated += *duration;
        let cs = (accumulated + 5) / 10;
        out.push(cs.saturating_sub(previous_cs) as u32);
        previous_cs = cs;
    }
    out
}

/// Escape text for an ASS Dialogue body.
pub fn escape_ass_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' | '\r' => out.push_str("\\N"),
            _ => out.push(c),
        }
    }
    out
}

fn sanitize_style_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == ',' || c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "DejaVu Sans".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[derive(Debug)]
struct Segment<'a> {
    tokens: Vec<(&'a Token, u64, u64)>,
}

/// Build a complete `.ass` file for the project.
///
/// Each current line is one bottom-anchored event. When the next line is within
/// the on-deck lead window, it is included above the current line:
///
/// ```text
/// {\rNext}next line\N{\rKaraoke}{\kf50}current...
/// ```
///
/// The event ends at the current line's explicit/inferred end. If there is an
/// instrumental gap, a separate `Next`-style on-deck event appears at most six
/// seconds before the next line starts.
pub fn build_ass(project: &Project) -> String {
    let style = &project.style;
    let (canvas_width, canvas_height) = project.export.aspect.dimensions();
    let side_margin = project.export.aspect.side_margin();
    let next_color = rgb_to_ass(style.next_line_color());
    let font = sanitize_style_name(&style.font_family);
    // Dots and title-card text use the bundled DejaVu face so the glyphs are
    // always present and match the preview's bundled lyric font.
    let countdown_font = crate::assets::FONT_FAMILY;
    let preroll_ms = project.preroll_ms();
    let title_color = rgb_to_ass(style.highlight_color);
    let mut out = String::new();
    out.push_str("; KaraokeKit karaoke subtitles\n");
    out.push_str("[Script Info]\n");
    out.push_str("ScriptType: v4.00+\n");
    out.push_str(&format!(
        "Title: {}\n",
        escape_ass_text(project.title.trim())
    ));
    out.push_str(&format!("PlayResX: {canvas_width}\n"));
    out.push_str(&format!("PlayResY: {canvas_height}\n"));
    out.push_str("WrapStyle: 0\n");
    out.push_str("ScaledBorderAndShadow: yes\n");
    out.push_str("YCbCr Matrix: TV.601\n\n");

    out.push_str("[V4+ Styles]\n");
    out.push_str(
        "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
         BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, \
         BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
    );
    out.push_str(&format!(
        "Style: Karaoke,{font},{:.1},{},{},&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,{:.1},{:.1},2,{side_margin},{side_margin},{:.1},1\n",
        style.font_size,
        // Sung highlight first, unsung base second (libass karaoke convention).
        rgb_to_ass(style.highlight_color),
        rgb_to_ass(style.base_color),
        style.outline_width,
        style.shadow_width,
        style.margin_v,
    ));
    out.push_str(&format!(
        "Style: Next,{font},{:.1},{next_color},{next_color},&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,{:.1},{:.1},2,{side_margin},{side_margin},{:.1},1\n",
        style.font_size,
        style.outline_width,
        style.shadow_width,
        style.margin_v,
    ));
    out.push_str(&format!(
        "Style: Countdown,{countdown_font},{:.1},{},{},&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,0.0,0.0,2,{side_margin},{side_margin},{:.1},1\n",
        style.font_size * crate::model::COUNTDOWN_DOT_SCALE,
        rgb_to_ass(style.highlight_color),
        rgb_to_ass(style.highlight_color),
        style.margin_v,
    ));
    out.push_str(&format!(
        "Style: Title,{countdown_font},{:.1},{title_color},{title_color},&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,{:.1},{:.1},5,{side_margin},{side_margin},0,1\n",
        style.font_size * crate::model::PREROLL_TITLE_SCALE,
        style.outline_width,
        style.shadow_width,
    ));
    out.push_str(&format!(
        "Style: Artist,{countdown_font},{:.1},{next_color},{next_color},&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,{:.1},{:.1},5,{side_margin},{side_margin},0,1\n\n",
        style.font_size * crate::model::PREROLL_ARTIST_SCALE,
        style.outline_width,
        style.shadow_width,
    ));

    out.push_str("[Events]\n");
    out.push_str(
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );

    let spans = project.spans();
    let intervals = project.line_intervals();
    let mut interval_by_line: Vec<Option<crate::model::LineInterval>> =
        vec![None; project.lines.len()];
    for interval in &intervals {
        interval_by_line[interval.index] = Some(*interval);
    }
    let mut next_line_of: Vec<Option<usize>> = vec![None; project.lines.len()];
    for (position, interval) in intervals.iter().enumerate() {
        if let Some(next) = intervals.get(position + 1) {
            next_line_of[interval.index] = Some(next.index);
        }
    }

    let dialogue = |out: &mut String, start: u64, end: u64, style_name: &str, text: &str| {
        if end <= start {
            return;
        }
        out.push_str(&format!(
            "Dialogue: 0,{},{},{style_name},,0,0,0,,{}\n",
            ass_time(start),
            ass_time(end),
            text
        ));
    };

    // Title card + pre-roll cue, or the ordinary intro when there is no pre-roll.
    if let Some(first) = intervals.first().copied() {
        if preroll_ms > 0 {
            let title_x = canvas_width as i32 / 2;
            let title_y = (canvas_height as f32 * crate::model::PREROLL_TITLE_Y).round() as i32;
            let artist_y = (canvas_height as f32 * crate::model::PREROLL_ARTIST_Y).round() as i32;
            if !project.title.trim().is_empty() {
                dialogue(
                    &mut out,
                    0,
                    preroll_ms,
                    "Title",
                    &format!(
                        "{{\\an5\\pos({title_x},{title_y})}}{}",
                        escape_ass_text(project.title.trim())
                    ),
                );
            }
            if !project.artist.trim().is_empty() {
                dialogue(
                    &mut out,
                    0,
                    preroll_ms,
                    "Artist",
                    &format!(
                        "{{\\an5\\pos({title_x},{artist_y})}}{}",
                        escape_ass_text(project.artist.trim())
                    ),
                );
            }

            let first_text = escape_ass_text(&project.lines[first.index].text());
            for step in 0..4usize {
                let phase_start = preroll_ms * step as u64 / 4;
                let phase_end = preroll_ms * (step + 1) as u64 / 4;
                if phase_end <= phase_start {
                    continue;
                }
                let dots = crate::model::countdown_text(4 - step);
                dialogue(
                    &mut out,
                    phase_start,
                    phase_end,
                    "Next",
                    &format!("{{\\rCountdown}}{dots}\\N{{\\rNext}}{first_text}"),
                );
            }
            // After the title card, use the normal six-second on-deck lookahead
            // (without a second countdown sequence).
            emit_on_deck_event(&mut out, project, preroll_ms, first, false);
        } else {
            emit_on_deck_event(&mut out, project, 0, first, project.style.lead_in_countdown);
        }
    }

    let mut flat = 0usize;
    for (line_index, line) in project.lines.iter().enumerate() {
        let Some(interval) = interval_by_line[line_index] else {
            flat += line.tokens.len();
            continue;
        };
        let line_end = interval.end;
        let next_line_index = next_line_of[line_index];
        let next_interval = next_line_index.and_then(|index| interval_by_line[index]);
        let next_text = next_line_index
            .filter(|_| {
                next_interval
                    .map(|next| {
                        next.start <= line_end.saturating_add(crate::model::ON_DECK_LEAD_MS)
                    })
                    .unwrap_or(false)
            })
            .map(|index| project.lines[index].text());

        // Gather timed tokens, split into events on long pauses.
        let mut segments: Vec<Segment<'_>> = Vec::new();
        let mut current = Segment { tokens: Vec::new() };
        let mut previous_end: Option<u64> = None;
        for (index, token) in line.tokens.iter().enumerate() {
            let Some((start, end)) = spans.get(flat + index).copied().flatten() else {
                continue;
            };
            if let Some(prev_end) = previous_end {
                if start.saturating_sub(prev_end) > GAP_SPLIT_MS && !current.tokens.is_empty() {
                    segments.push(std::mem::replace(
                        &mut current,
                        Segment { tokens: Vec::new() },
                    ));
                }
            }
            previous_end = Some(end);
            current.tokens.push((token, start, end));
        }
        if !current.tokens.is_empty() {
            segments.push(current);
        }
        flat += line.tokens.len();

        for (segment_index, segment) in segments.iter().enumerate() {
            let segment_start = segment.tokens[0].1;
            let karaoke_end = segment
                .tokens
                .last()
                .map(|(_, _, end)| *end)
                .unwrap_or(segment_start);

            let durations: Vec<u64> = segment
                .tokens
                .iter()
                .enumerate()
                .map(|(i, (_, start, _))| {
                    let next = segment
                        .tokens
                        .get(i + 1)
                        .map(|(_, next_start, _)| *next_start)
                        .unwrap_or(karaoke_end);
                    next.saturating_sub(*start)
                        .max(if i + 1 == segment.tokens.len() { 20 } else { 0 })
                })
                .collect();
            let cs = karaoke_cs(&durations);

            let mut text = String::new();
            if let Some(next) = &next_text {
                text.push_str("{\\rNext}");
                text.push_str(&escape_ass_text(next));
                text.push_str("\\N{\\rKaraoke}");
            }
            for (i, (token, _, _)) in segment.tokens.iter().enumerate() {
                if i > 0 && token.word_start {
                    text.push(' ');
                }
                text.push_str(&format!(
                    "{{\\kf{}}}{}",
                    cs[i],
                    escape_ass_text(&token.text)
                ));
            }

            let mut event_end = segments
                .get(segment_index + 1)
                .and_then(|next| next.tokens.first().map(|(_, start, _)| *start))
                .unwrap_or(line_end)
                .min(line_end);
            // Avoid a one-frame double-render when the next line starts exactly
            // where this one ends.
            if let Some(next) = next_interval {
                if next.start <= event_end {
                    event_end = next.start.saturating_sub(10);
                }
            }
            event_end = event_end.max(segment_start + 10);
            dialogue(&mut out, segment_start, event_end, "Karaoke", &text);
        }
    }

    // On-deck + countdown events fill instrumental gaps, within the lead window.
    for pair in intervals.windows(2) {
        emit_on_deck_event(
            &mut out,
            project,
            pair[0].end,
            pair[1],
            project.style.lead_in_countdown,
        );
    }

    out
}

/// Emit the dim on-deck line, split into countdown steps when warranted.
fn emit_on_deck_event(
    out: &mut String,
    project: &Project,
    previous_end: u64,
    next: crate::model::LineInterval,
    countdown_enabled: bool,
) {
    let deck_start = previous_end.max(next.start.saturating_sub(crate::model::ON_DECK_LEAD_MS));
    if deck_start >= next.start {
        return;
    }
    let text = escape_ass_text(&project.lines[next.index].text());
    let gap = next.start.saturating_sub(previous_end);
    let countdown = countdown_enabled && gap >= crate::model::LEAD_IN_GAP_MS;
    let countdown_start = next.start.saturating_sub(crate::model::COUNTDOWN_WINDOW_MS);

    let pre_end = if countdown {
        countdown_start.saturating_sub(10)
    } else {
        next.start.saturating_sub(10)
    };
    if deck_start < pre_end {
        out.push_str(&format!(
            "Dialogue: 0,{},{},Next,,0,0,0,,{}\n",
            ass_time(deck_start),
            ass_time(pre_end),
            text
        ));
    }

    if !countdown {
        return;
    }

    for step in 0..4usize {
        let phase_start =
            countdown_start.saturating_add(step as u64 * crate::model::COUNTDOWN_STEP_MS);
        if phase_start >= next.start {
            break;
        }
        let phase_end = (phase_start + crate::model::COUNTDOWN_STEP_MS).min(next.start);
        if phase_end <= phase_start {
            break;
        }
        let dots = crate::model::countdown_text(4 - step);
        out.push_str(&format!(
            "Dialogue: 0,{},{},Next,,0,0,0,,{{\\rCountdown}}{dots}\\N{{\\rNext}}{text}\n",
            ass_time(phase_start),
            ass_time(phase_end)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::tokenize;

    fn timed_project() -> Project {
        let mut project = Project {
            title: "Cafeteria Song".to_owned(),
            ..Project::default()
        };
        project.style.font_size = 72.0;
        project.lines = tokenize("won/der/ful day");
        project.set_token_start(0, 1_000);
        project.set_token_start(1, 1_500);
        project.set_token_start(2, 1_750);
        project.set_token_start(3, 2_000);
        project.tail_ms = 900;
        project
    }

    #[test]
    fn formats_ass_timestamps() {
        assert_eq!(ass_time(0), "0:00:00.00");
        assert_eq!(ass_time(1_234), "0:00:01.23");
        assert_eq!(ass_time(3_661_230), "1:01:01.23");
    }

    #[test]
    fn rounds_kf_durations_cumulatively() {
        assert_eq!(karaoke_cs(&[500, 250, 125]), vec![50, 25, 13]);
        assert_eq!(karaoke_cs(&[0, 0, 0]), vec![0, 0, 0]);
        assert_eq!(karaoke_cs(&[4, 4, 4]), vec![0, 1, 0]);
    }

    #[test]
    fn emits_karaoke_tags_and_colours() {
        let ass = build_ass(&timed_project());
        assert!(ass.starts_with("; KaraokeKit"));
        assert!(ass.contains("[Script Info]"));
        assert!(ass.contains("\\kf50"));
        assert!(ass.contains("\\kf25"));
        assert!(ass.contains("\\kf90"));
        assert!(ass.contains("&H004AD5FF")); // highlight (sung)
        assert!(ass.contains("&H00FFFFFF")); // base (unsung)
        assert!(ass.contains("Dialogue: 0,0:00:01.00,0:00:02.90"));
        assert!(ass.contains("won"));
        assert!(!ass.contains("won/der/ful"));
    }

    fn two_line_project() -> Project {
        let mut project = Project {
            title: "Two Lines".to_owned(),
            ..Project::default()
        };
        project.lines = tokenize("first line here\nsecond line here");
        for (index, start) in [1_000, 1_300, 1_600, 2_000, 2_300, 2_600]
            .into_iter()
            .enumerate()
        {
            project.set_token_start(index, start);
        }
        project.tail_ms = 900;
        project
    }

    #[test]
    fn uses_selected_aspect_for_playres_and_margins() {
        let mut project = timed_project();
        let widescreen = build_ass(&project);
        assert!(widescreen.contains("PlayResX: 1920"));
        assert!(widescreen.contains("PlayResY: 1080"));
        assert!(widescreen.contains(",2,134,134,"));

        project.export.aspect = crate::model::AspectRatio::Classic;
        let classic = build_ass(&project);
        assert!(classic.contains("PlayResX: 1440"));
        assert!(classic.contains("PlayResY: 1080"));
        assert!(classic.contains(",2,101,101,"));
    }

    #[test]
    fn clears_instrumental_gaps_and_shows_on_deck_line() {
        let text = "[00:02.00]First verse has four words\n\
                    [00:06.00]\n\
                    [00:18.00]Second verse starts here\n";
        let document = crate::lrc::parse_lrc(text);
        let mut project = Project::default();
        crate::lrc::apply_lrc(&mut project, &document);
        // Baseline gap behavior: no optional countdown dots.
        project.style.lead_in_countdown = false;
        let ass = build_ass(&project);

        assert!(ass
            .contains("Dialogue: 0,0:00:00.00,0:00:01.99,Next,,0,0,0,,First verse has four words"));
        let first_current = ass
            .lines()
            .find(|line| line.contains("First") && line.contains("\\kf"))
            .expect("first current event");
        assert!(first_current.contains("0:00:02.00"));
        assert!(first_current.contains("0:00:06.00"));
        assert!(!first_current.contains("\\rNext"));
        assert!(!first_current.contains("0:00:18"));

        assert!(
            ass.contains("Dialogue: 0,0:00:12.00,0:00:17.99,Next,,0,0,0,,Second verse starts here")
        );
        let second_current = ass
            .lines()
            .find(|line| line.contains("Second") && line.contains("\\kf"))
            .expect("second current event");
        assert!(second_current.contains("0:00:18.00"));
    }

    #[test]
    fn far_upcoming_line_waits_for_the_lookahead_window() {
        let mut project = timed_project();
        for (index, start) in [12_000, 12_500, 12_750, 13_000].into_iter().enumerate() {
            project.set_token_start(index, start);
        }
        project.style.lead_in_countdown = false;
        let ass = build_ass(&project);
        assert!(ass.contains("Dialogue: 0,0:00:06.00,0:00:11.99,Next,,0,0,0,,wonderful day"));
        assert!(!ass.contains("Dialogue: 0,0:00:00.00,0:00:11.99,Next"));
    }

    fn countdown_project() -> Project {
        let document = crate::lrc::parse_lrc(
            "[00:02.00]First verse has four words\n\
             [00:06.00]\n\
             [00:18.00]Second verse starts here\n",
        );
        let mut project = Project::default();
        crate::lrc::apply_lrc(&mut project, &document);
        project
    }

    #[test]
    fn emits_four_step_countdown_for_long_gaps() {
        let project = countdown_project();
        assert!(project.style.lead_in_countdown);
        let ass = build_ass(&project);

        // The ordinary on-deck line appears first.
        assert!(
            ass.contains("Dialogue: 0,0:00:12.00,0:00:14.79,Next,,0,0,0,,Second verse starts here")
        );
        assert!(ass.contains(
            "Dialogue: 0,0:00:14.80,0:00:15.60,Next,,0,0,0,,{\\rCountdown}● ● ● ●\\N{\\rNext}Second verse starts here"
        ));
        assert!(ass.contains(
            "Dialogue: 0,0:00:15.60,0:00:16.40,Next,,0,0,0,,{\\rCountdown}● ● ●\\N{\\rNext}Second verse starts here"
        ));
        assert!(ass.contains(
            "Dialogue: 0,0:00:16.40,0:00:17.20,Next,,0,0,0,,{\\rCountdown}● ●\\N{\\rNext}Second verse starts here"
        ));
        assert!(ass.contains(
            "Dialogue: 0,0:00:17.20,0:00:18.00,Next,,0,0,0,,{\\rCountdown}●\\N{\\rNext}Second verse starts here"
        ));
        // Dots are gone once the next sweep starts.
        assert!(!ass
            .lines()
            .any(|line| line.starts_with("Dialogue: 0,0:00:18.00") && line.contains("●")));
    }

    #[test]
    fn short_gaps_and_disabled_setting_omit_countdown_dots() {
        let short = crate::lrc::parse_lrc(
            "[00:02.00]First verse has four words\n\
             [00:06.00]\n\
             [00:09.90]Second verse starts here\n",
        );
        let mut short_project = Project::default();
        crate::lrc::apply_lrc(&mut short_project, &short);
        let short_ass = build_ass(&short_project);
        assert!(!short_ass.contains("●"));

        let mut disabled = countdown_project();
        disabled.style.lead_in_countdown = false;
        let disabled_ass = build_ass(&disabled);
        assert!(!disabled_ass.contains("●"));
        assert!(disabled_ass
            .contains("Dialogue: 0,0:00:12.00,0:00:17.99,Next,,0,0,0,,Second verse starts here"));
    }

    #[test]
    fn splits_long_pauses_into_new_events() {
        let mut project = timed_project();
        project.set_token_end(0, Some(1_200));
        project.set_token_start(1, 2_500);
        project.set_token_start(2, 3_000);
        project.set_token_start(3, 3_250);
        let ass = build_ass(&project);
        let karaoke_events = ass
            .lines()
            .filter(|l| l.starts_with("Dialogue:") && l.contains("\\kf"))
            .count();
        assert_eq!(karaoke_events, 2);
    }

    #[test]
    fn burns_the_next_line_above_the_current_line() {
        let ass = build_ass(&two_line_project());
        assert!(ass.contains("Style: Next,"));
        assert!(ass.contains("{\\rNext}second line here\\N{\\rKaraoke}"));
        assert!(ass.contains("\\kf"));
        // The upcoming line's style uses the dimmed base colour (255 * 0.55).
        assert!(ass.contains("&H008C8C8C"));

        // The first line's event carries the next line; the last line has no prefix.
        let events: Vec<_> = ass
            .lines()
            .filter(|line| line.starts_with("Dialogue:"))
            .collect();
        let first_karaoke = events
            .iter()
            .find(|line| line.contains("first") && line.contains("\\kf"))
            .expect("first lyric event");
        assert!(first_karaoke.contains("{\\rNext}second line here"));
        let last_event = events
            .iter()
            .rev()
            .find(|line| line.contains("second") && line.contains("\\kf"))
            .expect("second lyric event");
        assert!(!last_event.contains("\\rNext"));
    }

    #[test]
    fn shows_a_dimmed_first_line_before_it_starts() {
        let ass = build_ass(&timed_project());
        assert!(ass.contains("Dialogue: 0,0:00:00.00,0:00:00.99,Next,,"));

        let mut early = timed_project();
        early.set_token_start(0, 0);
        early.set_token_start(1, 500);
        early.set_token_start(2, 750);
        early.set_token_start(3, 1_000);
        let early_ass = build_ass(&early);
        assert!(!early_ass.contains(",Next,,0,0,0,,"));
    }

    #[test]
    fn escapes_ass_control_characters() {
        assert_eq!(escape_ass_text("a{b}\\c"), "a\\{b\\}\\\\c");
        let mut project = timed_project();
        project.lines = tokenize("a{b}/c");
        project.set_token_start(0, 0);
        project.set_token_start(1, 500);
        let ass = build_ass(&project);
        assert!(ass.contains("\\{b\\}"));
    }
}
