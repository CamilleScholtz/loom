//! Structured interpretation of the player's free-form dialogue line.
//!
//! The npc-turn LLM call returns a single JSON object combining a structured
//! classification of the player's utterance with the NPC's spoken reply. The
//! classification is consumed by `crate::engine::interpret::apply` to produce
//! bounded relationship and memory deltas; the reply is shown to the player.
//!
//! The schema is fixed in code — the model picks from enum variants only, it
//! cannot invent fields. That preserves the GAME.md rule that the LLM voices
//! state but never decides it.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Ask,
    Inform,
    Confront,
    Comfort,
    Flirt,
    Insult,
    Threat,
    Accuse,
    SmallTalk,
    #[default]
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Hostile,
    Cold,
    #[default]
    Neutral,
    Warm,
    Playful,
    Earnest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RomanticSignal {
    Flirt,
    Confess,
    Rebuff,
    Reciprocate,
}

/// How the model interprets the truthfulness of an `AssertedFact`. Drives the
/// `Distortion` on the resulting `Event::Told` so the listener's knowledge
/// edge reflects how much to trust it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Truthfulness {
    #[default]
    Earnest,
    Speculative,
    Lying,
}

/// A fact the player is asserting in their line. The summary is plain prose
/// (no schema enforcement); `about` is an NpcId reference the engine validates
/// against the world — invalid ids are dropped, never invented.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertedFact {
    pub summary: String,
    #[serde(default)]
    pub about: Option<u32>,
    #[serde(default)]
    pub truthfulness: Truthfulness,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerLineInterpretation {
    #[serde(default)]
    pub intent: Intent,
    #[serde(default)]
    pub tone: Tone,
    #[serde(default)]
    pub romantic_signal: Option<RomanticSignal>,
    #[serde(default)]
    pub references: Vec<u32>,
    #[serde(default)]
    pub asserts: Vec<AssertedFact>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NpcTurnResponse {
    #[serde(default)]
    pub intent: Intent,
    #[serde(default)]
    pub tone: Tone,
    #[serde(default)]
    pub romantic_signal: Option<RomanticSignal>,
    #[serde(default)]
    pub references: Vec<u32>,
    #[serde(default)]
    pub asserts: Vec<AssertedFact>,
    #[serde(default)]
    pub reply: String,
}

/// JSON-schema describing the parameters of the `respond_in_character` tool
/// the model is forced to call. Co-located with the enums so adding a new
/// variant lights up the schema and the parser in one place.
pub fn tool_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "intent": {
                "type": "string",
                "enum": [
                    "ask", "inform", "confront", "comfort", "flirt",
                    "insult", "threat", "accuse", "small_talk", "other"
                ]
            },
            "tone": {
                "type": "string",
                "enum": ["hostile", "cold", "neutral", "warm", "playful", "earnest"]
            },
            "romantic_signal": {
                "type": ["string", "null"],
                "enum": ["flirt", "confess", "rebuff", "reciprocate", null]
            },
            "references": {
                "type": "array",
                "items": { "type": "integer", "minimum": 0 },
                "description": "NpcIds from the roster the line is about"
            },
            "asserts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "summary": { "type": "string" },
                        "about": { "type": ["integer", "null"], "minimum": 0 },
                        "truthfulness": {
                            "type": "string",
                            "enum": ["earnest", "speculative", "lying"]
                        }
                    },
                    "required": ["summary"]
                }
            },
            "reply": {
                "type": "string",
                "description": "The character's reply as one or two short lines"
            }
        },
        "required": ["reply"]
    })
}

/// Stable name of the tool the model must call. Co-located with `tool_schema`
/// for the same drift-prevention reason.
pub const TOOL_NAME: &str = "respond_in_character";

impl NpcTurnResponse {
    pub fn interpretation(&self) -> PlayerLineInterpretation {
        PlayerLineInterpretation {
            intent: self.intent,
            tone: self.tone,
            romantic_signal: self.romantic_signal,
            references: self.references.clone(),
            asserts: self.asserts.clone(),
        }
    }

    pub fn interpretation_take(self) -> (PlayerLineInterpretation, String) {
        let reply = self.reply;
        let interp = PlayerLineInterpretation {
            intent: self.intent,
            tone: self.tone,
            romantic_signal: self.romantic_signal,
            references: self.references,
            asserts: self.asserts,
        };
        (interp, reply)
    }
}

/// Parse the model's response. Tolerates the JSON being wrapped in markdown
/// code fences or surrounded by stray prose by extracting the first balanced
/// `{...}` block. Returns an `Err` only when no JSON object is found or it is
/// structurally invalid; unknown fields and missing optional fields are
/// accepted via `serde(default)`.
pub fn parse_npc_turn_response(raw: &str) -> Result<NpcTurnResponse> {
    let json = extract_json_block(raw).context("no JSON object found in response")?;
    let parsed: NpcTurnResponse =
        serde_json::from_str(json).with_context(|| format!("parsing JSON: {}", json))?;
    Ok(parsed)
}

/// Stream-aware extractor for the `reply` field of incoming tool args. Feed
/// in fragments; whenever it crosses the `"reply": "..."` boundary it returns
/// the chars that should be emitted to the UI. Survives across chunk
/// boundaries (escapes split across fragments are handled by the state).
#[derive(Default, Debug)]
pub struct ReplyTokenStreamer {
    buffer: String,
    state: ReplyState,
}

#[derive(Default, Debug, PartialEq, Eq)]
enum ReplyState {
    #[default]
    SearchingKey,
    AfterKey,
    InValue,
    EscapeInValue,
    Done,
}

impl ReplyTokenStreamer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a fragment of tool-args JSON. Returns any chars from `reply`
    /// that have been newly observed. Safe to call repeatedly; never
    /// returns the same char twice.
    pub fn push(&mut self, fragment: &str) -> String {
        self.buffer.push_str(fragment);
        let bytes = self.buffer.as_bytes();
        let mut emit = String::new();
        // We work char-by-char from `cursor` until we either finish the
        // reply value or run out of buffered bytes. `cursor` tracks how
        // much of `buffer` we have consumed; on each call we reset it to
        // 0 and re-walk, since the state itself remembers where we are.
        // Cheap because buffer is small.
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i] as char;
            match self.state {
                ReplyState::SearchingKey => {
                    if self.buffer[i..].starts_with("\"reply\"") {
                        self.state = ReplyState::AfterKey;
                        i += "\"reply\"".len();
                        continue;
                    }
                    i += 1;
                }
                ReplyState::AfterKey => {
                    // Skip whitespace and the colon, look for the opening
                    // quote of the value.
                    if c == '"' {
                        self.state = ReplyState::InValue;
                    }
                    i += 1;
                }
                ReplyState::InValue => match c {
                    '\\' => {
                        self.state = ReplyState::EscapeInValue;
                        i += 1;
                    }
                    '"' => {
                        self.state = ReplyState::Done;
                        i += 1;
                    }
                    _ => {
                        emit.push(c);
                        i += 1;
                    }
                },
                ReplyState::EscapeInValue => {
                    let unescaped = match c {
                        'n' => Some('\n'),
                        't' => Some('\t'),
                        'r' => Some('\r'),
                        '"' => Some('"'),
                        '\\' => Some('\\'),
                        '/' => Some('/'),
                        // `\uXXXX` is rare in NPC dialogue; emit literally
                        // and rely on the final parse to decode. Keeps the
                        // streamer simple.
                        _ => None,
                    };
                    if let Some(u) = unescaped {
                        emit.push(u);
                    }
                    self.state = ReplyState::InValue;
                    i += 1;
                }
                ReplyState::Done => break,
            }
        }
        // Drop everything we've consumed; keep only the unconsumed tail.
        self.buffer.drain(..i);
        emit
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, ReplyState::Done)
    }
}

fn extract_json_block(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    for i in start..bytes.len() {
        let c = bytes[i];
        if in_string {
            if escape {
                escape = false;
                continue;
            }
            if c == b'\\' {
                escape = true;
                continue;
            }
            if c == b'"' {
                in_string = false;
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_clean_json_blob() {
        let raw = r#"{"intent":"insult","tone":"hostile","reply":"Watch your tongue."}"#;
        let r = parse_npc_turn_response(raw).unwrap();
        assert_eq!(r.intent, Intent::Insult);
        assert_eq!(r.tone, Tone::Hostile);
        assert_eq!(r.reply, "Watch your tongue.");
        assert!(r.romantic_signal.is_none());
    }

    #[test]
    fn tolerates_markdown_code_fence_wrapping() {
        let raw = "```json\n{\"intent\":\"comfort\",\"tone\":\"warm\",\"reply\":\"I'm sorry.\"}\n```";
        let r = parse_npc_turn_response(raw).unwrap();
        assert_eq!(r.intent, Intent::Comfort);
        assert_eq!(r.tone, Tone::Warm);
    }

    #[test]
    fn tolerates_leading_or_trailing_prose() {
        let raw = "Here you go:\n{\"intent\":\"ask\",\"tone\":\"neutral\",\"reply\":\"Yes?\"} cheers";
        let r = parse_npc_turn_response(raw).unwrap();
        assert_eq!(r.intent, Intent::Ask);
        assert_eq!(r.reply, "Yes?");
    }

    #[test]
    fn missing_fields_default_to_other_neutral_empty() {
        let r = parse_npc_turn_response(r#"{"reply":"hm."}"#).unwrap();
        assert_eq!(r.intent, Intent::Other);
        assert_eq!(r.tone, Tone::Neutral);
        assert_eq!(r.reply, "hm.");
    }

    #[test]
    fn no_json_at_all_is_an_error() {
        let err = parse_npc_turn_response("just prose, no braces").unwrap_err();
        assert!(format!("{:#}", err).contains("no JSON"));
    }

    #[test]
    fn nested_braces_inside_strings_do_not_confuse_extractor() {
        let raw = r#"{"intent":"inform","tone":"neutral","reply":"He said \"{secret}\" and left."}"#;
        let r = parse_npc_turn_response(raw).unwrap();
        assert_eq!(r.reply, r#"He said "{secret}" and left."#);
    }

    #[test]
    fn romantic_signal_parses_when_present() {
        let raw = r#"{"intent":"flirt","tone":"playful","romantic_signal":"flirt","reply":"Hush."}"#;
        let r = parse_npc_turn_response(raw).unwrap();
        assert_eq!(r.romantic_signal, Some(RomanticSignal::Flirt));
    }

    #[test]
    fn references_and_asserts_parse_with_defaults() {
        let raw = r#"{
            "intent":"inform",
            "tone":"earnest",
            "references":[3,7],
            "asserts":[
                {"summary":"Brother Hew was at the well at midnight","about":3,"truthfulness":"earnest"},
                {"summary":"Aldwin paid in silver","truthfulness":"speculative"}
            ],
            "reply":"As you say."
        }"#;
        let r = parse_npc_turn_response(raw).unwrap();
        assert_eq!(r.references, vec![3, 7]);
        assert_eq!(r.asserts.len(), 2);
        assert_eq!(r.asserts[0].about, Some(3));
        assert_eq!(r.asserts[0].truthfulness, Truthfulness::Earnest);
        assert_eq!(r.asserts[1].about, None);
        assert_eq!(r.asserts[1].truthfulness, Truthfulness::Speculative);
    }

    #[test]
    fn unknown_truthfulness_falls_back_to_default() {
        // serde will fail to parse "shaky" — outer parser should bubble that up
        // as a parse error, since truthfulness is enum-typed.
        let raw = r#"{"asserts":[{"summary":"x","truthfulness":"shaky"}],"reply":""}"#;
        assert!(parse_npc_turn_response(raw).is_err());
    }

    #[test]
    fn missing_references_and_asserts_default_to_empty() {
        let r = parse_npc_turn_response(r#"{"reply":"yes"}"#).unwrap();
        assert!(r.references.is_empty());
        assert!(r.asserts.is_empty());
    }

    #[test]
    fn reply_streamer_emits_chars_after_key_boundary() {
        let mut s = ReplyTokenStreamer::new();
        let out = s.push(r#"{"intent":"ask","reply":"Hello there.""#);
        assert_eq!(out, "Hello there.");
        assert!(s.is_done());
    }

    #[test]
    fn reply_streamer_handles_fragmented_chunks() {
        let mut s = ReplyTokenStreamer::new();
        let mut collected = String::new();
        for chunk in [r#"{"intent":"ask""#, r#","reply":"H"#, r#"ello"#, r#" there.""#, "}"] {
            collected.push_str(&s.push(chunk));
        }
        assert_eq!(collected, "Hello there.");
        assert!(s.is_done());
    }

    #[test]
    fn reply_streamer_handles_escaped_quote_inside_value() {
        let mut s = ReplyTokenStreamer::new();
        let out = s.push(r#"{"reply":"He said \"hush\" and left.""#);
        assert_eq!(out, r#"He said "hush" and left."#);
        assert!(s.is_done());
    }

    #[test]
    fn reply_streamer_handles_escape_split_across_chunks() {
        let mut s = ReplyTokenStreamer::new();
        let a = s.push(r#"{"reply":"He said \"#);
        // Without the closing quote of the escape, the streamer should
        // emit "He said " and pause waiting for the escape resolution.
        assert_eq!(a, "He said ");
        let b = s.push(r#""hush.""#);
        assert_eq!(b, r#""hush."#);
        assert!(s.is_done());
    }

    #[test]
    fn reply_streamer_ignores_other_string_fields_before_reply() {
        let mut s = ReplyTokenStreamer::new();
        let out = s.push(r#"{"intent":"ask","tone":"warm","reply":"hi.""#);
        assert_eq!(out, "hi.");
    }
}
