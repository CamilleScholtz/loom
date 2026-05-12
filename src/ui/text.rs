//! Helpers for rendering dialogue prose that may contain inline `*action*`
//! segments. The convention: any text between matched `*` characters is a
//! narrative action / emote ("she leans against the bulkhead"), shown in
//! italic+dim so it reads as motion-and-gesture distinct from the speech
//! around it.
//!
//! The parser is intentionally simple: each `*` toggles in/out of an action
//! segment, asterisks themselves are stripped from the output, and an
//! unmatched closing `*` at the end is tolerated by treating the rest of
//! the line as plain speech. The renderer composes a list of styled spans
//! the caller drops into a `ratatui::text::Line`.

use ratatui::style::Style;
use ratatui::text::Span;

use super::palette;

/// One chunk of a dialogue line as parsed by [`split_segments`]. The
/// asterisks that delimited the action have already been stripped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment<'a> {
    /// Plain speech / narration — render in the speaker's normal color.
    Speech(&'a str),
    /// An emote / action between `*asterisks*` — render via [`palette::action`].
    Action(&'a str),
}

/// Walk `line` and split it into alternating `Speech` and `Action` segments.
/// Empty segments are dropped, so consecutive asterisks (`**`) collapse to
/// nothing rather than producing a zero-length action.
pub fn split_segments(line: &str) -> Vec<Segment<'_>> {
    let mut out: Vec<Segment<'_>> = Vec::new();
    let mut start = 0usize;
    let mut in_action = false;
    for (i, c) in line.char_indices() {
        if c != '*' {
            continue;
        }
        let chunk = &line[start..i];
        if !chunk.is_empty() {
            out.push(if in_action {
                Segment::Action(chunk)
            } else {
                Segment::Speech(chunk)
            });
        }
        start = i + c.len_utf8();
        in_action = !in_action;
    }
    let tail = &line[start..];
    if !tail.is_empty() {
        // An unmatched trailing `*` would leave `in_action=true` but no
        // closing delimiter. Treat the rest as speech in that case — the
        // model probably typed an unbalanced quote and we'd rather render
        // it than swallow the line.
        out.push(if in_action {
            Segment::Speech(tail)
        } else {
            Segment::Speech(tail)
        });
    }
    out
}

/// Convert a dialogue `line` into styled spans suitable for a `Line`. Plain
/// speech is rendered with `speech_style`; `*emotes*` are restyled via
/// [`palette::action`]. The caller is responsible for the speaker-name
/// prefix span — this function only handles the body text.
pub fn dialogue_spans(line: &str, speech_style: Style) -> Vec<Span<'_>> {
    split_segments(line)
        .into_iter()
        .map(|seg| match seg {
            Segment::Speech(s) => Span::styled(s, speech_style),
            Segment::Action(s) => Span::styled(s, palette::action()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_plain_speech_unchanged() {
        let segs = split_segments("Hello there.");
        assert_eq!(segs, vec![Segment::Speech("Hello there.")]);
    }

    #[test]
    fn splits_pure_action() {
        let segs = split_segments("*she leans against the bulkhead*");
        assert_eq!(segs, vec![Segment::Action("she leans against the bulkhead")]);
    }

    #[test]
    fn splits_mixed_speech_and_action() {
        let segs = split_segments("*she sighs* You really want to know?");
        assert_eq!(
            segs,
            vec![
                Segment::Action("she sighs"),
                Segment::Speech(" You really want to know?"),
            ]
        );
    }

    #[test]
    fn splits_multiple_action_segments() {
        let segs = split_segments("Look, *she taps the panel* *then waves you off* — go.");
        assert_eq!(
            segs,
            vec![
                Segment::Speech("Look, "),
                Segment::Action("she taps the panel"),
                Segment::Speech(" "),
                Segment::Action("then waves you off"),
                Segment::Speech(" — go."),
            ]
        );
    }

    #[test]
    fn empty_action_collapses() {
        // `**` toggles on and immediately off with no content; we drop the
        // empty segment rather than emit an Action("").
        let segs = split_segments("Hush **— they're listening.");
        assert_eq!(
            segs,
            vec![
                Segment::Speech("Hush "),
                Segment::Speech("— they're listening."),
            ]
        );
    }

    #[test]
    fn unmatched_trailing_asterisk_falls_back_to_speech() {
        let segs = split_segments("She said it was over*");
        assert_eq!(
            segs,
            vec![Segment::Speech("She said it was over")],
        );
    }

    #[test]
    fn empty_string_yields_no_segments() {
        assert!(split_segments("").is_empty());
    }

    #[test]
    fn handles_utf8_inside_actions() {
        let segs = split_segments("*she rolls her eyes — pointedly* Fine.");
        assert_eq!(
            segs,
            vec![
                Segment::Action("she rolls her eyes — pointedly"),
                Segment::Speech(" Fine."),
            ]
        );
    }

    #[test]
    fn dialogue_spans_applies_action_style_distinct_from_speech() {
        let segs = dialogue_spans(
            "*she sighs* Fine.",
            Style::default(),
        );
        assert_eq!(segs.len(), 2);
        // The action span carries the action style; the speech span does not.
        let action_style = palette::action();
        assert_eq!(segs[0].style, action_style);
        assert_ne!(segs[1].style, action_style);
    }
}
