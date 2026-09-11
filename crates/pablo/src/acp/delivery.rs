//! One notification in flight, acknowledged by the actual consumer.
use super::*;

#[allow(dead_code)] // Consumed by the C3.21 internal host; enabled supervision follows at C3.22.
pub(super) struct TypedUpdate {
    pub notification: wire::AgentNotification,
    pub consumed: tokio::sync::oneshot::Sender<()>,
}

pub(super) enum Delivery {
    Typed {
        sender: async_channel::Sender<TypedUpdate>,
        closed: CancellationToken,
    },
    Stdio {
        cx: ConnectionTo<Client>,
        written: Arc<Written>,
    },
}
impl Delivery {
    pub(super) async fn send(&self, notification: wire::AgentNotification) -> Result<(), Error> {
        match self {
            Self::Typed { sender, closed } => {
                let (consumed, receipt) = tokio::sync::oneshot::channel();
                let delivery = tokio::time::timeout(WRITE_TIMEOUT, async {
                    sender
                        .send(TypedUpdate {
                            notification,
                            consumed,
                        })
                        .await
                        .map_err(|_| Error::internal_error())?;
                    receipt.await.map_err(|_| Error::internal_error())
                });
                tokio::select! {
                    result = delivery => result.map_err(|_| Error::internal_error())?,
                    _ = closed.cancelled() => Err(Error::internal_error()),
                }
            }
            Self::Stdio { cx, written } => {
                let expected = written.count.load(Ordering::Acquire) + 1;
                cx.send_notification(notification)?;
                while written.count.load(Ordering::Acquire) < expected {
                    written.changed.notified().await;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn typed_delivery_waits_for_consumption_and_close_wakes_unacknowledged_work() {
        let (sender, receiver) = async_channel::bounded(1);
        let closed = CancellationToken::new();
        let delivery = Delivery::Typed {
            sender,
            closed: closed.clone(),
        };
        let notification = || {
            wire::AgentNotification::SessionNotification(wire::SessionNotification::new(
                "session",
                wire::SessionUpdate::AgentMessageChunk(wire::ContentChunk::new(
                    wire::ContentBlock::Text(wire::TextContent::new("typed")),
                )),
            ))
        };
        let first = delivery.send(notification());
        tokio::pin!(first);
        tokio::select! {
            result = &mut first => panic!("delivery completed before consumption: {result:?}"),
            update = receiver.recv() => { update.unwrap().consumed.send(()).unwrap(); }
        }
        first.await.unwrap();
        let second = delivery.send(notification());
        tokio::pin!(second);
        let held = tokio::select! {
            result = &mut second => panic!("delivery completed before consumption: {result:?}"),
            update = receiver.recv() => update.unwrap(),
        };
        closed.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), second)
                .await
                .unwrap()
                .is_err()
        );
        drop(held);
    }
}
