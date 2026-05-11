use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use openrouter_rs::OpenRouterClient as SdkClient;
use openrouter_rs::api::chat::{ChatCompletionRequest, Message as SdkMessage};
use openrouter_rs::types::{Role as SdkRole, Tool as SdkTool};
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::sync::oneshot;

use super::stream::{StreamChunk, StreamReceiver, StreamSender};
use super::{ChatRequest, Client, Role};

enum WorkerJob {
    Chat {
        request: ChatRequest,
        reply: oneshot::Sender<Result<String>>,
    },
    Stream {
        request: ChatRequest,
        reply: oneshot::Sender<Result<StreamReceiver>>,
    },
    Shutdown,
}

pub struct OpenRouterClientImpl {
    tx: mpsc::Sender<WorkerJob>,
    handle: Option<JoinHandle<()>>,
}

impl OpenRouterClientImpl {
    pub fn new(api_key: String) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<WorkerJob>();

        let handle = thread::Builder::new()
            .name("llm-worker".into())
            .spawn(move || worker_loop(api_key, rx))
            .context("spawning llm worker thread")?;

        Ok(Self {
            tx,
            handle: Some(handle),
        })
    }
}

impl Drop for OpenRouterClientImpl {
    fn drop(&mut self) {
        let _ = self.tx.send(WorkerJob::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Client for OpenRouterClientImpl {
    fn chat(&self, request: ChatRequest) -> Result<String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(WorkerJob::Chat {
                request,
                reply: reply_tx,
            })
            .map_err(|_| anyhow!("llm worker thread is gone"))?;
        reply_rx
            .blocking_recv()
            .map_err(|_| anyhow!("llm worker dropped reply"))?
    }

    fn chat_stream(&self, request: ChatRequest) -> Result<StreamReceiver> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(WorkerJob::Stream {
                request,
                reply: reply_tx,
            })
            .map_err(|_| anyhow!("llm worker thread is gone"))?;
        reply_rx
            .blocking_recv()
            .map_err(|_| anyhow!("llm worker dropped reply"))?
    }
}

fn worker_loop(api_key: String, rx: mpsc::Receiver<WorkerJob>) {
    // Multi-thread runtime so a long-running stream can be spawned as a task
    // and the worker can immediately accept the next job.
    let runtime = match RuntimeBuilder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("building tokio runtime in llm worker")
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "llm worker failed to start runtime");
            return;
        }
    };

    let sdk = match SdkClient::builder()
        .api_key(api_key)
        .build()
        .context("building openrouter client")
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "llm worker failed to build sdk client");
            return;
        }
    };

    while let Ok(job) = rx.recv() {
        match job {
            WorkerJob::Shutdown => break,
            WorkerJob::Chat { request, reply } => {
                let result = runtime.block_on(send_chat(&sdk, request));
                let _ = reply.send(result);
            }
            WorkerJob::Stream { request, reply } => {
                let (chunk_tx, chunk_rx) = mpsc::channel::<StreamChunk>();
                // Hand the receiver back immediately; the caller starts
                // draining tokens while the stream is still in flight.
                if reply.send(Ok(chunk_rx)).is_err() {
                    continue;
                }
                let sdk = sdk.clone();
                runtime.spawn(async move {
                    drive_stream(&sdk, request, chunk_tx).await;
                });
            }
        }
    }
}

async fn send_chat(sdk: &SdkClient, req: ChatRequest) -> Result<String> {
    let chat_req = build_chat_request(req)?;
    let response = sdk
        .chat()
        .create(&chat_req)
        .await
        .context("openrouter chat completion")?;
    let content = response
        .choices
        .first()
        .and_then(|c| c.content())
        .ok_or_else(|| anyhow!("openrouter returned no content"))?
        .to_string();
    Ok(content)
}

async fn drive_stream(sdk: &SdkClient, req: ChatRequest, tx: StreamSender) {
    let chat_req = match build_chat_request(req) {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(StreamChunk::Error(format!("{:#}", e)));
            let _ = tx.send(StreamChunk::Done);
            return;
        }
    };
    let mut stream = match sdk.chat().stream(&chat_req).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(StreamChunk::Error(format!("openrouter stream: {}", e)));
            let _ = tx.send(StreamChunk::Done);
            return;
        }
    };
    while let Some(item) = stream.next().await {
        match item {
            Ok(resp) => {
                let choice = match resp.choices.first() {
                    Some(c) => c,
                    None => continue,
                };
                // Plain content fragment (no tool attached, or model emitted
                // prose alongside the tool call — rare but legal).
                if let Some(c) = choice.content() {
                    if !c.is_empty()
                        && tx.send(StreamChunk::Token(c.to_string())).is_err()
                    {
                        return;
                    }
                }
                // Tool-call fragments: each chunk's `function.arguments`
                // carries a slice of the JSON args. Forward as ToolArgs.
                if let Some(partials) = choice.partial_tool_calls() {
                    for p in partials {
                        if let Some(args) = p.function.as_ref().and_then(|f| f.arguments.as_ref()) {
                            if !args.is_empty()
                                && tx.send(StreamChunk::ToolArgs(args.clone())).is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(StreamChunk::Error(format!("openrouter chunk: {}", e)));
            }
        }
    }
    let _ = tx.send(StreamChunk::Done);
}

fn build_chat_request(req: ChatRequest) -> Result<ChatCompletionRequest> {
    let messages: Vec<SdkMessage> = req
        .messages
        .into_iter()
        .map(|m| SdkMessage::new(role_to_sdk(m.role), m.content))
        .collect();
    let mut builder = ChatCompletionRequest::builder();
    builder.model(req.model).messages(messages);
    if let Some(tool) = req.tool {
        let sdk_tool = SdkTool::new(&tool.name, &tool.description, tool.parameters_schema);
        builder.tool(sdk_tool);
        if tool.force {
            builder.force_tool(&tool.name);
        }
    }
    builder.build().context("building chat completion request")
}

fn role_to_sdk(role: Role) -> SdkRole {
    match role {
        Role::System => SdkRole::System,
        Role::User => SdkRole::User,
        Role::Assistant => SdkRole::Assistant,
    }
}
