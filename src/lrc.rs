//! LRC import/export with instrumental-gap awareness.
//!
//! Plain LRC only gives line start times. A blank timestamp line
//! (`[mm:ss.xx]` with no text) is treated as an explicit end boundary for the
//! preceding lyric line. Without a marker, inferred word starts are kept inside a
//! natural budget (`800 ms × syllables`) and the final syllable is capped at
//! 1.2 s, so a verse before a long solo does not stretch across it.

use crate::lyrics::tokenize_line_with_offsets;
use crate::model::{LyricLine, Project, Token};
use crate::util::format_time;

pub const MAX_WORD_MS: u64 = 800;
pub const MAX_LINE_TAIL_MS: u64 = 1_200;

#[derive(Debug, Default)]
pub struct LrcDocument {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub offset_ms: i64,
    pub lines: Vec<LyricLine>,
}

fn parse_timestamp(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let (minutes, rest) = raw.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;
    let seconds: f64 = rest.trim().parse().ok()?;
    if !seconds.is_finite() || seconds < 0.0 || seconds >= 60.0 {
        return None;
    }
    Some(((minutes as f64 * 60.0 + seconds) * 1000.0).round() as u64)
}

#[derive(Debug)]
struct RawLine {
    start: u64,
    content: String,
    /// `(byte offset after the tag, absolute ms)`.
    tags: Vec<(usize, u64)>,
    is_marker: bool,
    end_marker: Option<u64>,
}

/// Split `<mm:ss.cc>` word tags out of the content, replacing them with spaces so
/// byte offsets stay aligned with the original text.
fn extract_word_tags(content: &str) -> (String, Vec<(usize, u64)>) {
    let mut cleaned = content.as_bytes().to_vec();
    let mut tags = Vec::new();
    let mut i = 0usize;
    while i < cleaned.len() {
        if cleaned[i] == b'<' {
            if let Some(relative_end) = content[i..].find('>') {
                let end = i + relative_end;
                if let Some(time) = parse_timestamp(&content[i + 1..end]) {
                    for byte in &mut cleaned[i..=end] {
                        if *byte != b'\n' {
                            *byte = b' ';
                        }
                    }
                    tags.push((end + 1, time));
                    i = end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    (String::from_utf8(cleaned).unwrap_or_default(), tags)
}

pub fn parse_lrc(text: &str) -> LrcDocument {
    let mut doc = LrcDocument::default();
    let mut raws: Vec<RawLine> = Vec::new();

    for raw_line in text.lines() {
        let mut rest = raw_line.trim_start();
        let mut stamps = Vec::new();
        loop {
            if !rest.starts_with('[') {
                break;
            }
            let Some(close) = rest.find(']') else { break };
            let inside = &rest[1..close];
            if let Some(start) = parse_timestamp(inside) {
                stamps.push(start);
                rest = rest[close + 1..].trim_start();
                continue;
            }
            if let Some((key, value)) = inside.split_once(':') {
                match key.trim().to_ascii_lowercase().as_str() {
                    "ti" => doc.title = Some(value.trim().to_owned()),
                    "ar" => doc.artist = Some(value.trim().to_owned()),
                    "offset" => doc.offset_ms = value.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
            rest = &rest[close + 1..];
        }

        let content = rest.trim().to_owned();
        if stamps.is_empty() {
            continue;
        }
        let (cleaned, tags) = extract_word_tags(&content);
        let is_marker = cleaned.trim().is_empty();
        for start in stamps {
            raws.push(RawLine {
                start,
                content: cleaned.clone(),
                tags: tags.clone(),
                is_marker,
                end_marker: None,
            });
        }
    }

    let shift = |ms: u64| (ms as i64 + doc.offset_ms).max(0) as u64;
    for raw in &mut raws {
        raw.start = shift(raw.start);
        raw.end_marker = raw.end_marker.map(shift);
        for tag in &mut raw.tags {
            tag.1 = shift(tag.1);
        }
    }
    raws.sort_by_key(|raw| raw.start);

    // Blank timestamp rows end the previous non-blank line (first marker wins).
    let mut last_lyric: Option<usize> = None;
    for index in 0..raws.len() {
        if raws[index].is_marker {
            if let Some(previous) = last_lyric {
                if raws[previous].end_marker.is_none() {
                    raws[previous].end_marker = Some(raws[index].start);
                }
            }
        } else {
            last_lyric = Some(index);
        }
    }

    // Tokenize lyric rows first so empty/punctuation-only rows don't become
    // "next lines" for duration calculations.
    let mut candidates: Vec<(usize, Vec<(Token, usize)>)> = Vec::new();
    for (index, raw) in raws.iter().enumerate() {
        if raw.is_marker {
            continue;
        }
        let tokens = tokenize_line_with_offsets(&raw.content);
        if !tokens.is_empty() {
            candidates.push((index, tokens));
        }
    }

    for (position, (raw_index, tokens_with_offsets)) in candidates.iter().enumerate() {
        let raw = &raws[*raw_index];
        let line_start = raw.start;
        let next_start = candidates
            .get(position + 1)
            .map(|(index, _)| raws[*index].start);
        let token_count = tokens_with_offsets.len() as u64;
        let natural_budget = MAX_WORD_MS.saturating_mul(token_count).max(1);
        let available = next_start
            .map(|next| next.saturating_sub(line_start))
            .unwrap_or(u64::MAX);
        let explicit_end = raw.end_marker.filter(|end| *end > line_start).map(|end| {
            next_start
                .map(|next| end.min(next))
                .unwrap_or(end)
                .max(line_start + 10)
        });

        let mut budget = natural_budget.min(available).max(1);
        if let Some(end) = explicit_end {
            budget = budget.min(end.saturating_sub(line_start).max(1));
        }
        let natural_end = line_start.saturating_add(budget);
        let display_end = explicit_end.unwrap_or(natural_end).max(line_start + 10);
        let distribution_end = natural_end.min(display_end);

        let mut tokens: Vec<Token> = tokens_with_offsets
            .iter()
            .map(|(token, _)| token.clone())
            .collect();

        // Assign every token to the last word tag at or before it.
        let mut group_of_token = Vec::with_capacity(tokens_with_offsets.len());
        for (_, byte) in tokens_with_offsets {
            let group = raw
                .tags
                .iter()
                .rposition(|(tag_byte, _)| *tag_byte <= *byte)
                .map(|group| group as isize)
                .unwrap_or(-1);
            group_of_token.push(group);
        }

        let group_count = raw.tags.len() + 1;
        let mut ranges = vec![(usize::MAX, 0usize); group_count];
        for (index, group) in group_of_token.iter().enumerate() {
            let group_index = (*group + 1) as usize;
            if ranges[group_index].0 == usize::MAX {
                ranges[group_index].0 = index;
            }
            ranges[group_index].1 = index + 1;
        }

        for (group_index, &(first, last)) in ranges.iter().enumerate() {
            if first >= last {
                continue;
            }
            let group_start = if group_index == 0 {
                line_start
            } else {
                raw.tags[group_index - 1]
                    .1
                    .clamp(line_start, distribution_end)
            };
            let group_end = raw
                .tags
                .get(group_index)
                .map(|(_, time)| *time)
                .unwrap_or(distribution_end)
                .clamp(group_start + 1, distribution_end.max(group_start + 1));

            let weights: Vec<f64> = tokens[first..last]
                .iter()
                .map(|token| token.text.chars().count().max(1) as f64)
                .collect();
            let total: f64 = weights.iter().sum();
            let mut consumed = 0.0;
            for offset in 0..(last - first) {
                let fraction = if total > 0.0 {
                    consumed / total
                } else {
                    offset as f64 / (last - first) as f64
                };
                let start = group_start as f64 + fraction * (group_end - group_start) as f64;
                tokens[first + offset].start_ms = Some(start.round() as u64);
                consumed += weights[offset];
            }
        }

        // Cap the last syllable's tail and, when a blank marker exists, let the
        // line remain visible until that explicit boundary.
        if let Some(last) = tokens.last_mut() {
            if let Some(last_start) = last.start_ms {
                let capped = last_start
                    .saturating_add(MAX_LINE_TAIL_MS)
                    .min(display_end)
                    .max(last_start + 10);
                last.end_ms = Some(capped);
            }
        }

        doc.lines.push(LyricLine {
            tokens,
            end_ms: explicit_end.map(|_| display_end),
        });
    }

    doc.lines.retain(|line| !line.tokens.is_empty());
    doc
}

/// Import an LRC into a project, replacing lyrics and applying its starter timings.
pub fn apply_lrc(project: &mut Project, doc: &LrcDocument) {
    project.lines = doc.lines.clone();
    project.tap_offset_ms = 0;
    if let Some(title) = &doc.title {
        if !title.trim().is_empty() {
            project.title = title.clone();
        }
    }
    if let Some(artist) = &doc.artist {
        project.artist = artist.clone();
    }
}

pub fn export_lrc(project: &Project) -> String {
    let mut out = String::new();
    if !project.title.trim().is_empty() {
        out.push_str(&format!("[ti:{}]\n", project.title.trim()));
    }
    if !project.artist.trim().is_empty() {
        out.push_str(&format!("[ar:{}]\n", project.artist.trim()));
    }
    if !out.is_empty() {
        out.push('\n');
    }

    // LRC timestamps are relative to the audio, not the title-card pre-roll.
    // `line_intervals()` works on the visible timeline, so subtract the
    // pre-roll when writing (and re-importing still applies it exactly once).
    let preroll = project.preroll_ms();
    let intervals = project.line_intervals();
    for (position, interval) in intervals.iter().enumerate() {
        let line = &project.lines[interval.index];
        out.push_str(&format!(
            "[{}]{}\n",
            format_time(interval.start.saturating_sub(preroll)),
            line.text()
        ));
        if line.end_ms.is_some()
            && intervals
                .get(position + 1)
                .map(|next| interval.end < next.start)
                .unwrap_or(false)
        {
            // Preserve an explicit end boundary on round-trip.
            out.push_str(&format!(
                "[{}]\n",
                format_time(interval.end.saturating_sub(preroll))
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Project, ON_DECK_LEAD_MS};

    fn gap_lrc() -> &'static str {
        "[ti:Gap Test]\n\
         [ar:Test Artist]\n\
         [00:02.00]First verse has four words\n\
         [00:06.00]\n\
         [00:18.00]Second verse starts here\n"
    }

    #[test]
    fn parses_plain_lrc_and_metadata() {
        let doc = parse_lrc(
            "[ti:Cafeteria Song]\n\
             [ar:Lunchroom Legends]\n\
             [00:04.20]Trays are clat/ter/ing\n\
             [00:09.00]Mys/ter/y meat/loaf\n",
        );
        assert_eq!(doc.title.as_deref(), Some("Cafeteria Song"));
        assert_eq!(doc.artist.as_deref(), Some("Lunchroom Legends"));
        assert_eq!(doc.lines.len(), 2);
        assert_eq!(doc.lines[0].tokens[0].start_ms, Some(4_200));
        let starts: Vec<Option<u64>> = doc.lines[0]
            .tokens
            .iter()
            .map(|token| token.start_ms)
            .collect();
        assert!(starts.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn parses_enhanced_word_timestamps() {
        let doc = parse_lrc("[00:10.00]<00:10.00>Caf<00:10.30>e<00:10.60>te/ria\n");
        assert_eq!(doc.lines.len(), 1);
        assert_eq!(doc.lines[0].tokens[0].start_ms, Some(10_000));
        assert!(doc.lines[0]
            .tokens
            .iter()
            .all(|token| token.start_ms.is_some()));
        let starts: Vec<u64> = doc.lines[0]
            .tokens
            .iter()
            .map(|token| token.start_ms.unwrap())
            .collect();
        assert!(starts.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn parses_multiple_timestamps_on_one_line() {
        let doc = parse_lrc("[00:01.00][00:11.00]la la\n");
        assert_eq!(doc.lines.len(), 2);
        assert_eq!(doc.lines[0].tokens[0].start_ms, Some(1_000));
        assert_eq!(doc.lines[1].tokens[0].start_ms, Some(11_000));
    }

    #[test]
    fn blank_marker_ends_the_previous_line_and_clears_the_gap() {
        let doc = parse_lrc(gap_lrc());
        assert_eq!(doc.lines.len(), 2);
        assert_eq!(doc.lines[0].end_ms, Some(6_000));

        let mut project = Project::default();
        apply_lrc(&mut project, &doc);
        assert_eq!(project.line_span(0), Some((2_000, 6_000)));
        assert_eq!(project.line_span(1).map(|span| span.0), Some(18_000));

        // Mid-gap (6s..18s) there is no active line.
        assert_eq!(project.active_line(12_000), None);
        assert_eq!(project.on_deck_line(11_999, ON_DECK_LEAD_MS), None);
        assert_eq!(project.on_deck_line(12_000, ON_DECK_LEAD_MS), Some(1));
        assert_eq!(project.active_line(18_000), Some(1));

        // Token ends stay inside the explicit line end.
        let first_line_token_count = project.lines[0].tokens.len();
        for span in project
            .spans()
            .iter()
            .take(first_line_token_count)
            .flatten()
        {
            assert!(span.1 <= 6_000, "token end escaped the line: {span:?}");
        }
        let last = project.lines[0].tokens.last().unwrap();
        let last_start = last.start_ms.unwrap();
        assert!(last.end_ms.unwrap() <= last_start + MAX_LINE_TAIL_MS);
    }

    #[test]
    fn long_gap_without_marker_uses_a_bounded_end() {
        let doc = parse_lrc(
            "[00:02.00]First verse has four words\n\
             [00:18.00]Second verse starts here\n",
        );
        let mut project = Project::default();
        apply_lrc(&mut project, &doc);
        let (start, end) = project.line_span(0).expect("first line");
        assert_eq!(start, 2_000);
        assert!(end < 18_000, "line stretched into the solo: {end}");
        assert!(end <= start + MAX_WORD_MS * project.lines[0].tokens.len() as u64 + 200);
        assert_eq!(project.active_line(12_000), None);
        assert_eq!(project.on_deck_line(12_000, ON_DECK_LEAD_MS), Some(1));
    }

    #[test]
    fn exports_line_starts_and_explicit_end_markers() {
        let doc = parse_lrc(gap_lrc());
        let mut project = Project::default();
        apply_lrc(&mut project, &doc);
        let lrc = export_lrc(&project);
        assert!(lrc.contains("[ti:Gap Test]"));
        assert!(lrc.contains("[00:02.00]First verse has four words"));
        assert!(lrc.contains("[00:06.00]"));
        assert!(lrc.contains("[00:18.00]Second verse starts here"));
    }

    #[test]
    fn exports_audio_relative_times_with_preroll_and_round_trips() {
        let doc = parse_lrc(gap_lrc());
        let mut project = Project::default();
        apply_lrc(&mut project, &doc);
        project.style.intro_preroll_secs = 3.5;
        assert_eq!(project.preroll_ms(), 3_500);
        assert_eq!(project.line_span(0).map(|span| span.0), Some(5_500));

        let lrc = export_lrc(&project);
        assert!(lrc.contains("[ti:Gap Test]"));
        assert!(lrc.contains("[00:02.00]First verse has four words"));
        assert!(lrc.contains("[00:06.00]"));
        assert!(lrc.contains("[00:18.00]Second verse starts here"));
        assert!(!lrc.contains("[00:05.50]"));
        assert!(!lrc.contains("[00:09.50]"));

        // Re-importing into a project that still has the pre-roll must apply it
        // exactly once, not twice.
        let round_trip = parse_lrc(&lrc);
        let mut reopened = project.clone();
        apply_lrc(&mut reopened, &round_trip);
        assert_eq!(reopened.preroll_ms(), 3_500);
        assert_eq!(reopened.line_span(0), project.line_span(0));
        assert_eq!(reopened.line_span(1), project.line_span(1));
    }

    #[test]
    fn offset_shifts_lines_and_markers() {
        let doc = parse_lrc("[offset:-500]\n[00:10.00]one two\n[00:12.00]\n[00:20.00]three four\n");
        let mut project = Project::default();
        apply_lrc(&mut project, &doc);
        assert_eq!(project.line_span(0), Some((9_500, 11_500)));
        assert_eq!(project.line_span(1).map(|span| span.0), Some(19_500));
    }
}
