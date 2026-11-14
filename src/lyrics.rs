//! Lyric tokenization.
//!
//! `/` splits a word into sweepable syllables (`won/der/ful`); whitespace splits
//! words. Each syllable becomes one token, which is what the tap-sync editor and
//! `.ass` `\kf` generator operate on. LRC import/export lives in [`crate::lrc`].

use crate::model::{LyricLine, Token};

/// Tokenize one physical line, retaining the byte offset of each token inside it.
pub(crate) fn tokenize_line_with_offsets(line: &str) -> Vec<(Token, usize)> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < line.len() {
        let ch = line[cursor..].chars().next().unwrap();
        if ch.is_whitespace() {
            cursor += ch.len_utf8();
            continue;
        }
        let word_start_byte = cursor;
        while cursor < line.len() {
            let c = line[cursor..].chars().next().unwrap();
            if c.is_whitespace() {
                break;
            }
            cursor += c.len_utf8();
        }
        let word = &line[word_start_byte..cursor];
        let mut syllable_start = 0usize;
        let mut first = true;
        for (i, c) in word.char_indices() {
            if c == '/' {
                if i > syllable_start {
                    out.push((
                        Token {
                            text: word[syllable_start..i].to_owned(),
                            word_start: first,
                            start_ms: None,
                            end_ms: None,
                        },
                        word_start_byte + syllable_start,
                    ));
                    first = false;
                }
                syllable_start = i + c.len_utf8();
            }
        }
        if syllable_start < word.len() {
            out.push((
                Token {
                    text: word[syllable_start..].to_owned(),
                    word_start: first,
                    start_ms: None,
                    end_ms: None,
                },
                word_start_byte + syllable_start,
            ));
        }
    }
    out
}

/// Split pasted lyrics into lines of syllable tokens. Blank lines are dropped.
pub fn tokenize(text: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let tokens: Vec<Token> = tokenize_line_with_offsets(raw)
            .into_iter()
            .map(|(token, _)| token)
            .collect();
        if !tokens.is_empty() {
            lines.push(LyricLine {
                tokens,
                end_ms: None,
            });
        }
    }
    lines
}

/// Rebuild readable text from tokens (spaces only appear at word starts).
pub fn join_tokens(tokens: &[Token]) -> String {
    let mut out = String::new();
    for (i, token) in tokens.iter().enumerate() {
        if i > 0 && token.word_start {
            out.push(' ');
        }
        out.push_str(&token.text);
    }
    out
}

/// Rebuild a line while keeping syllable boundaries visible for editing.
pub fn join_tokens_with_slashes(tokens: &[Token]) -> String {
    let mut out = String::new();
    for (i, token) in tokens.iter().enumerate() {
        if i > 0 {
            if token.word_start {
                out.push(' ');
            } else {
                out.push('/');
            }
        }
        out.push_str(&token.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_slashes_into_syllables() {
        let lines = tokenize("won/der/ful day\nAmazing grace");
        assert_eq!(lines.len(), 2);
        let words: Vec<&str> = lines[0].tokens.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(words, vec!["won", "der", "ful", "day"]);
        let flags: Vec<bool> = lines[0].tokens.iter().map(|t| t.word_start).collect();
        assert_eq!(flags, vec![true, false, false, true]);
        assert_eq!(lines[1].tokens.len(), 2);
    }

    #[test]
    fn handles_unicode_and_empty_syllables() {
        let lines = tokenize("café//so/ci/ety\n\n  ");
        assert_eq!(lines.len(), 1);
        let words: Vec<&str> = lines[0].tokens.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(words, vec!["café", "so", "ci", "ety"]);
        assert!(lines[0].tokens[0].word_start);
        assert!(!lines[0].tokens[1].word_start);
    }

    #[test]
    fn winter_wrap_up_and_cafeteria_song_tokenize() {
        let lines = tokenize("Win/ter Wrap Up\nCaf/e/ter/ia song");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text(), "Winter Wrap Up");
        assert_eq!(lines[1].text(), "Cafeteria song");
    }

    #[test]
    fn joins_tokens_back_to_text() {
        let lines = tokenize("win/ter wrap/-up");
        assert_eq!(lines[0].text(), "winter wrap-up");
        assert_eq!(
            join_tokens_with_slashes(&lines[0].tokens),
            "win/ter wrap/-up"
        );
    }
}
