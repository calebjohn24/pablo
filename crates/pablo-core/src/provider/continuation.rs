//! Private task-owned wire state. It is never a public event or transcript item.
use serde_json::Value;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ContinuationScope {
    pub provider: &'static str,
    pub endpoint: String,
    pub requested_model: String,
    pub resolved_model: String,
    pub revision: &'static str,
    pub profile: &'static str,
}

/// Deliberately no Serialize/Deserialize. Even content-enabled traces and Debug
/// cannot acquire private model state through this carrier.
#[derive(Clone)]
pub struct Continuation {
    pub(crate) scope: ContinuationScope,
    pub(crate) items: Vec<Value>,
    bytes: usize,
}
impl std::fmt::Debug for Continuation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Continuation([private])")
    }
}
impl Continuation {
    pub(crate) fn new(scope: ContinuationScope, items: Vec<Value>, maximum: usize) -> Option<Self> {
        let bytes = serde_json::to_vec(&items).ok()?.len();
        (bytes <= maximum).then_some(Self {
            scope,
            items,
            bytes,
        })
    }
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }
}

/// Associates a private output projection with one assistant history entry.
/// The runtime creates entries only after successful stream completion.
pub struct ContinuationEntry {
    pub(crate) message_index: usize,
    pub(crate) value: Continuation,
}
impl std::fmt::Debug for ContinuationEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContinuationEntry([private])")
    }
}
