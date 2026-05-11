use std::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamChunk {
    /// A fragment of plain assistant content (legacy non-tool-use path).
    Token(String),
    /// A fragment of the forced tool call's `arguments` JSON. Consumers
    /// accumulate these and parse the result as `NpcTurnResponse` once `Done`
    /// arrives. Multiple fragments may be needed before the JSON is valid.
    ToolArgs(String),
    Done,
    Error(String),
}

pub type StreamReceiver = mpsc::Receiver<StreamChunk>;
pub type StreamSender = mpsc::Sender<StreamChunk>;
