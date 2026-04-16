//! Streaming protocol for workflow execution.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

/// Events emitted during workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamEvent {
    /// Workflow execution started.
    Start { workflow_id: String },

    /// Progress within a node.
    Progress {
        node_id: String,
        event_type: ProgressType,
        data: Value,
    },

    /// Workflow completed successfully.
    Done { output: Value },

    /// Workflow failed.
    Error {
        code: String,
        message: String,
        retryable: bool,
    },
}

/// Type of progress event — allows frontends to distinguish
/// text streaming from tool execution from status updates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProgressType {
    /// LLM token chunk.
    TextDelta,
    /// Tool invocation starting — data contains tool name + resolved args.
    ToolStart,
    /// Tool invocation complete — data contains result summary + hash.
    ToolEnd,
    /// Status update ("assembling context", "searching memory").
    Status,
}

/// Sender half of a stream channel.
pub type StreamSender = mpsc::Sender<StreamEvent>;

/// Receiver half of a stream channel.
pub type StreamReceiver = mpsc::Receiver<StreamEvent>;

/// Create a new stream channel with the given buffer size.
pub fn stream_channel(buffer: usize) -> (StreamSender, StreamReceiver) {
    mpsc::channel(buffer)
}

/// Collect all events from a stream into the final output value.
/// Returns the `Done` event's output, or an error if the stream
/// ends with an `Error` event or closes without `Done`.
pub async fn collect_stream(mut rx: StreamReceiver) -> Result<Value, String> {
    let mut last_output = None;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Done { output } => {
                last_output = Some(output);
            }
            StreamEvent::Error { message, .. } => {
                return Err(message);
            }
            _ => {}
        }
    }
    last_output.ok_or_else(|| "stream closed without Done event".to_string())
}

/// Collect text deltas from a stream into a single string.
pub async fn collect_text_stream(mut rx: StreamReceiver) -> Result<String, String> {
    let mut text = String::new();
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Progress {
                event_type: ProgressType::TextDelta,
                data,
                ..
            } => {
                if let Some(s) = data.as_str() {
                    text.push_str(s);
                }
            }
            StreamEvent::Error { message, .. } => {
                return Err(message);
            }
            StreamEvent::Done { .. } => break,
            _ => {}
        }
    }
    Ok(text)
}
