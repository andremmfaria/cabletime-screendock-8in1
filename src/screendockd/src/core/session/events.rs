use serde_json::Value;

/// An event the reader forwards to the controller.
#[derive(Debug)]
pub(super) enum DockEvent {
    Ready,
    Reply {
        id: u64,
        result: Option<Value>,
        error: Option<Value>,
    },
}
