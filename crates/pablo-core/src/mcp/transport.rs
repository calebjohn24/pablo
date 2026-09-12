//! Finite stdio framing around the official SDK's protocol machinery.
use super::{MAX_FRAME_BYTES, MAX_RESULT_BYTES};
use rmcp::{
    RoleClient,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
};
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug)]
pub struct FramingError;
impl std::fmt::Display for FramingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MCP transport failed")
    }
}
impl std::error::Error for FramingError {}

#[derive(Default)]
pub(super) struct Bounds {
    failure: AtomicU8,
    messages: AtomicUsize,
    progress_bytes: AtomicUsize,
}
impl Bounds {
    pub(super) fn failed(&self) -> bool {
        self.failure.load(Ordering::Acquire) != 0
    }
    pub(super) fn fail(&self) {
        self.failure.store(1, Ordering::Release);
    }
    pub(super) fn reset(&self) {
        self.messages.store(0, Ordering::Release);
        self.progress_bytes.store(0, Ordering::Release);
    }
    pub(super) fn admit(&self, value: &serde_json::Value, bytes: usize) -> bool {
        if self.messages.fetch_add(1, Ordering::AcqRel) >= 65 {
            return false;
        }
        if value.get("method").is_some() {
            // No server-initiated requests, resource updates or catalog mutation in this cut.
            value.get("id").is_none()
                && value["method"] == "notifications/progress"
                && bytes <= 1024
                && self
                    .progress_bytes
                    .fetch_add(bytes, Ordering::AcqRel)
                    .saturating_add(bytes)
                    <= 8192
        } else {
            value.get("id").is_some()
                && value["id"].as_str().is_none_or(|id| id.len() <= 128)
                && (value.get("result").is_some() ^ value.get("error").is_some())
        }
    }
}

pub(super) struct BoundedTransport<R, W> {
    reader: BufReader<R>,
    writer: Arc<Mutex<Option<W>>>,
    buffer: Vec<u8>,
    pub(super) bounds: Arc<Bounds>,
    pub(super) cancellation: CancellationToken,
}
impl<R: AsyncRead, W> BoundedTransport<R, W> {
    pub(super) fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::with_capacity(8192, reader),
            writer: Arc::new(Mutex::new(Some(writer))),
            buffer: Vec::new(),
            bounds: Arc::default(),
            cancellation: CancellationToken::new(),
        }
    }
}
impl<R, W> Transport<RoleClient> for BoundedTransport<R, W>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    type Error = FramingError;
    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), FramingError>> + Send + 'static {
        let writer = self.writer.clone();
        let cancellation = self.cancellation.clone();
        let bounds = self.bounds.clone();
        async move {
            if !crate::filesystem::fits(&item, MAX_RESULT_BYTES) {
                bounds.fail();
                return Err(FramingError);
            }
            let mut bytes = serde_json::to_vec(&item).map_err(|_| FramingError)?;
            // Each host request starts one bounded receive window; notifications do not reset it.
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| FramingError)?;
            if value.get("id").is_some() {
                bounds.reset();
            }
            bytes.push(b'\n');
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => Err(FramingError),
                result = async {
                    let mut guard = writer.lock().await;
                    let output = guard.as_mut().ok_or(FramingError)?;
                    output.write_all(&bytes).await.map_err(|_| FramingError)?;
                    output.flush().await.map_err(|_| FramingError)
                } => result,
            };
            if result.is_err() {
                bounds.fail();
            }
            result
        }
    }
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        loop {
            let available = tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return None,
                result = self.reader.fill_buf() => match result {
                    Ok(bytes) if !bytes.is_empty() => bytes,
                    Ok(_) => {if !self.buffer.is_empty() {self.bounds.fail();} return None;},
                    Err(_) => { self.bounds.fail(); return None; }
                }
            };
            let newline = available.iter().position(|b| *b == b'\n');
            let take = newline.map_or(available.len(), |i| i + 1);
            if self.buffer.len().saturating_add(take) > MAX_FRAME_BYTES {
                self.bounds.fail();
                return None;
            }
            self.buffer.extend_from_slice(&available[..take]);
            self.reader.consume(take);
            if newline.is_some() {
                let result = (|| {
                    let value: serde_json::Value = serde_json::from_slice(&self.buffer).ok()?;
                    if !self.bounds.admit(&value, self.buffer.len()) {
                        return None;
                    }
                    serde_json::from_value(value).ok()
                })();
                self.buffer.clear();
                if result.is_none() {
                    self.bounds.fail();
                }
                return result;
            }
        }
    }
    async fn close(&mut self) -> Result<(), FramingError> {
        self.cancellation.cancel();
        // Dropping the writer closes stdin; no peer cooperation is needed.
        self.writer.lock().await.take();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn k01_bounded_sdk_frame_mutations_always_settle_within_owned_memory() {
        let seed = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n";
        for bytes in crate::protocol_properties::mutations(seed) {
            let mut transport =
                BoundedTransport::new(std::io::Cursor::new(bytes), tokio::io::sink());
            let result = transport.receive().await;
            assert!(transport.buffer.len() <= MAX_FRAME_BYTES);
            if result.is_some() {
                assert!(!transport.bounds.failed());
            }
            transport.close().await.unwrap();
            assert!(transport.cancellation.is_cancelled());
        }
    }
    #[tokio::test]
    async fn framing_handles_fragmentation_and_rejects_unterminated_overflow() {
        let (host, mut peer) = tokio::io::duplex(8192);
        let (reader, writer) = tokio::io::split(host);
        let mut transport = BoundedTransport::new(reader, writer);
        let task = tokio::spawn(async move {
            for part in [
                b"{\"jsonrpc\":\"2.0\",\"id\":1,".as_slice(),
                b"\"result\":{}}\n",
            ] {
                peer.write_all(part).await.unwrap();
            }
            let _ = peer.write_all(&vec![b'x'; MAX_FRAME_BYTES + 1]).await;
        });
        assert!(transport.receive().await.is_some());
        assert!(transport.receive().await.is_none());
        assert!(transport.bounds.failed());
        assert!(transport.buffer.len() <= MAX_FRAME_BYTES);
        drop(transport);
        task.await.unwrap();
    }
    #[tokio::test]
    async fn malformed_frames_and_unsolicited_authority_requests_fail_closed() {
        for wire in [
            "{invalid}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"roots/list\"}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n",
        ] {
            let mut transport = BoundedTransport::new(wire.as_bytes(), tokio::io::sink());
            assert!(transport.receive().await.is_none());
            assert!(transport.bounds.failed());
        }
    }
}
