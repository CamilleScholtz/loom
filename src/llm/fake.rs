use std::sync::mpsc;

use anyhow::Result;

use super::stream::{StreamChunk, StreamReceiver};
use super::{ChatRequest, Client};

#[derive(Default, Debug)]
pub struct FakeClient;

impl FakeClient {
    pub fn new() -> Self {
        Self
    }
}

fn stub_reply(req: &ChatRequest) -> String {
    // Forced-tool requests need schema-conforming JSON so dry-run exercises
    // the parser path. We branch on the tool name; everything else falls
    // through to the free-form stub.
    if let Some(tool) = req.tool.as_ref() {
        if tool.name == super::builders::OPTIONS_TOOL_NAME {
            return stub_options_json(&tool.parameters_schema);
        }
    }
    let is_npc_turn = req
        .messages
        .iter()
        .any(|m| m.content.contains(super::builders::NPC_TURN_SCHEMA_MARKER));
    let last = req
        .messages
        .last()
        .map(|m| m.content.as_str())
        .unwrap_or("");
    if is_npc_turn {
        return stub_npc_turn_json(last);
    }
    let truncated: String = last.chars().take(40).collect();
    format!("[stub reply to {:?}]", truncated)
}

/// Build a schema-shaped `{"lines": [...]}` blob using the count baked into
/// the request's tool schema (`properties.lines.minItems`). Dry-run mode then
/// exercises the parser and the count-validation path without hitting the wire.
fn stub_options_json(schema: &serde_json::Value) -> String {
    let count = schema
        .get("properties")
        .and_then(|p| p.get("lines"))
        .and_then(|l| l.get("minItems"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0) as usize;
    let lines: Vec<String> = (0..count).map(|i| format!("[stub option {}]", i)).collect();
    serde_json::json!({ "lines": lines }).to_string()
}

/// Coarse keyword classifier so the FakeClient's "interpretation" tracks the
/// shape of the player's line in dry-run mode. Real models do real
/// classification; this only exists so integration tests and dry-run
/// playthroughs exercise the full engine path.
fn stub_npc_turn_json(player_line: &str) -> String {
    let lc = player_line.to_lowercase();
    let (intent, tone) = if lc.contains("fool")
        || lc.contains("idiot")
        || lc.contains("liar")
        || lc.contains("damn you")
    {
        ("insult", "hostile")
    } else if lc.contains("sorry") || lc.contains("i'm here") || lc.contains("rest") {
        ("comfort", "warm")
    } else if lc.contains("love") || lc.contains("beautiful") || lc.contains("kiss") {
        ("flirt", "playful")
    } else if lc.contains("did you") || lc.contains("where were") || lc.contains("?") {
        ("ask", "neutral")
    } else {
        ("small_talk", "neutral")
    };
    let truncated: String = player_line.chars().take(40).collect();
    let reply = format!("[stub reply to {:?}]", truncated);
    serde_json::json!({
        "intent": intent,
        "tone": tone,
        "romantic_signal": serde_json::Value::Null,
        "reply": reply,
    })
    .to_string()
}

impl Client for FakeClient {
    fn chat(&self, req: ChatRequest) -> Result<String> {
        Ok(stub_reply(&req))
    }

    fn chat_stream(&self, req: ChatRequest) -> Result<StreamReceiver> {
        let (tx, rx) = mpsc::channel();
        let reply = stub_reply(&req);
        let is_tool_call = req.tool.is_some();
        // Push tokens on a small thread with a brief sleep between each so
        // dry-run mode actually exercises the streaming path of the UI rather
        // than racing every token into the channel before the first draw.
        std::thread::spawn(move || {
            if is_tool_call {
                // Tool-mode: chunk the JSON args into ~6-char fragments so
                // the streaming JSON path-watcher (in the consumer) has
                // something to chew on.
                let bytes: Vec<char> = reply.chars().collect();
                for chunk in bytes.chunks(6) {
                    let s: String = chunk.iter().collect();
                    if tx.send(StreamChunk::ToolArgs(s)).is_err() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            } else {
                for word in reply.split_whitespace() {
                    if tx.send(StreamChunk::Token(format!("{} ", word))).is_err() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            }
            let _ = tx.send(StreamChunk::Done);
        });
        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatMessage, Role};

    fn req(prompt: &str) -> ChatRequest {
        ChatRequest {
            model: "stub".into(),
            messages: vec![ChatMessage::new(Role::User, prompt)],
            tool: None,
        }
    }

    #[test]
    fn chat_is_deterministic() {
        let c = FakeClient::new();
        let a = c.chat(req("hello there")).unwrap();
        let b = c.chat(req("hello there")).unwrap();
        assert_eq!(a, b);
        assert!(a.contains("hello there"));
    }

    #[test]
    fn chat_stream_emits_tokens_then_done() {
        let c = FakeClient::new();
        let rx = c.chat_stream(req("hello world from rust")).unwrap();
        let chunks: Vec<StreamChunk> = rx.iter().collect();
        assert!(chunks.len() >= 2);
        assert_eq!(chunks.last().unwrap(), &StreamChunk::Done);
        for chunk in &chunks[..chunks.len() - 1] {
            assert!(matches!(chunk, StreamChunk::Token(_)));
        }
    }

    fn npc_turn_req(player_line: &str) -> ChatRequest {
        // Mimic the shape `npc_turn_request` produces: the marker lives in
        // the system message, the user message carries the player's line.
        ChatRequest {
            model: "stub".into(),
            messages: vec![
                ChatMessage::new(
                    Role::System,
                    format!("npc voice stuff {}", super::super::builders::NPC_TURN_SCHEMA_MARKER),
                ),
                ChatMessage::new(Role::User, player_line),
            ],
            tool: None,
        }
    }

    #[test]
    fn npc_turn_stub_classifies_insults_and_returns_parseable_json() {
        let c = FakeClient::new();
        let raw = c.chat(npc_turn_req("you fool")).unwrap();
        let parsed = crate::llm::interpret::parse_npc_turn_response(&raw).unwrap();
        assert_eq!(parsed.intent, crate::llm::interpret::Intent::Insult);
        assert_eq!(parsed.tone, crate::llm::interpret::Tone::Hostile);
        assert!(!parsed.reply.is_empty());
    }

    #[test]
    fn npc_turn_stub_survives_player_line_with_quotes() {
        let c = FakeClient::new();
        let raw = c.chat(npc_turn_req("she said \"liar\" and left")).unwrap();
        // The point: parses cleanly despite embedded quotes — JSON escaping
        // is handled by serde_json, not by hand-rolled string concatenation.
        let _ = crate::llm::interpret::parse_npc_turn_response(&raw).unwrap();
    }
}
