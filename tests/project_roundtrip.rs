//! Fixture-driven integration tests for project persistence, LRC import, and ASS output.

use karaoke_kit::ass;
use karaoke_kit::lrc;
use karaoke_kit::model::{Project, ON_DECK_LEAD_MS};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn winter_wrap_up_project_round_trips() {
    let project = Project::load_json(&fixture("winter_wrap_up.json")).expect("load fixture");
    assert_eq!(project.title, "Winter Wrap Up");
    assert_eq!(project.artist, "The Weather Team");
    assert_eq!(
        project.instrumental.as_deref(),
        Some(Path::new("winter_wrap_up.flac"))
    );
    assert_eq!(
        project.vocal.as_deref(),
        Some(Path::new("winter_wrap_up_vocals.flac"))
    );
    assert_eq!(project.token_count(), 12);
    assert!(project.all_tokens_timed());
    // Projects written before aspect support default to 16:9.
    assert_eq!(
        project.export.aspect,
        karaoke_kit::model::AspectRatio::Widescreen
    );

    let json = project.to_json().expect("serialize");
    let back = Project::from_json(&json).expect("deserialize");
    assert_eq!(project, back);

    // The first syllable spans until the next token starts.
    assert_eq!(project.token_span(0), Some((1_200, 1_650)));
    assert_eq!(project.token_span(4), Some((2_600, 3_400)));
}

#[test]
fn instrumental_gap_lrc_clears_the_line_and_burns_on_deck() {
    let text = std::fs::read_to_string(fixture("instrumental_gap.lrc")).expect("gap lrc fixture");
    let document = lrc::parse_lrc(&text);
    let mut project = Project::default();
    lrc::apply_lrc(&mut project, &document);
    project.style.lead_in_countdown = false;

    assert_eq!(project.line_span(0), Some((2_000, 6_000)));
    assert_eq!(project.line_span(1).map(|span| span.0), Some(18_000));
    assert_eq!(project.active_line(12_000), None);
    assert_eq!(project.on_deck_line(12_000, ON_DECK_LEAD_MS), Some(1));
    assert_eq!(project.active_line(18_000), Some(1));

    let ass_text = ass::build_ass(&project);
    assert!(ass_text.contains("0:00:12.00,0:00:17.99,Next"));
    assert!(ass_text.lines().any(|line| line.contains("Second")
        && line.contains("\\kf")
        && line.contains("0:00:18.00")));
}

#[test]
fn cafeteria_lrc_imports_and_exports() {
    let text = std::fs::read_to_string(fixture("cafeteria_song.lrc")).expect("lrc fixture");
    let document = lrc::parse_lrc(&text);
    assert_eq!(document.title.as_deref(), Some("Cafeteria Song"));
    assert_eq!(document.artist.as_deref(), Some("The Lunchroom Legends"));
    assert_eq!(document.lines.len(), 5);

    let mut project = Project {
        instrumental: Some(PathBuf::from("winter_wrap_up.flac")),
        ..Project::default()
    };
    lrc::apply_lrc(&mut project, &document);

    assert_eq!(project.title, "Cafeteria Song");
    assert!(project.token_count() >= 30);
    assert!(project.all_tokens_timed());

    // The imported plain-LRC line starts at its timestamp.
    let first_line_start = project.line_span(0).map(|(start, _)| start);
    assert_eq!(first_line_start, Some(4_200));

    // ASS generation must produce progressive-fill tags for the imported song.
    let ass_text = ass::build_ass(&project);
    assert!(ass_text.contains("\\kf"));
    assert!(ass_text.contains("Dialogue:"));
    assert!(!ass_text.contains("not tapped"));

    // Plain LRC export keeps line-level timestamps and readable words.
    let lrc_text = lrc::export_lrc(&project);
    assert!(lrc_text.contains("[ti:Cafeteria Song]"));
    assert!(lrc_text.contains("[00:04.20]Trays are clattering in the hall"));
}
