//! One notification in flight, acknowledged by the actual consumer.
use super::*;

#[allow(dead_code)] // Consumed by the C3.21 internal host; enabled supervision follows at C3.22.
pub(super) struct TypedUpdate {
    pub notification: wire::AgentNotification,
    pub consumed: tokio::sync::oneshot::Sender<()>,
}

/// Original runtime record, before ACP projection or text coalescing.
/// Acknowledgement covers consumption by the root's bounded native stream.
#[cfg_attr(not(test), allow(dead_code))] // Root consumer wiring is the next C3.22 integration step.
pub(super) struct NativeUpdate {
    pub event: RunEvent,
    pub consumed: tokio::sync::oneshot::Sender<()>,
}

pub(super) enum Delivery {
    Native {
        sender: async_channel::Sender<NativeUpdate>,
        closed: CancellationToken,
    },
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
    pub(super) async fn forward_native(
        &self,
        events: async_channel::Receiver<RunEvent>,
    ) -> Result<Option<RunEvent>, Error> {
        let Self::Native { sender, closed } = self else {
            return Err(Error::internal_error());
        };
        let mut terminal = None;
        loop {
            let event = tokio::select! {
                event = events.recv() => match event { Ok(event) => event, Err(_) => break },
                _ = closed.cancelled() => return Err(Error::internal_error()),
                _ = sender.closed() => return Err(Error::internal_error()),
            };
            if matches!(event.kind, EventKind::RunFinished { .. }) {
                terminal = Some(event.clone());
            }
            let (consumed, receipt) = tokio::sync::oneshot::channel();
            let delivery = tokio::time::timeout(WRITE_TIMEOUT, async {
                sender
                    .send(NativeUpdate { event, consumed })
                    .await
                    .map_err(|_| Error::internal_error())?;
                receipt.await.map_err(|_| Error::internal_error())
            });
            tokio::select! {
                biased;
                result = delivery => result.map_err(|_| Error::internal_error())??,
                _ = closed.cancelled() => return Err(Error::internal_error()),
                _ = sender.closed() => return Err(Error::internal_error()),
            }
            // The terminal acknowledgement is the last delivery obligation.
            // PendingPrompt still awaits worker settlement and owned cleanup.
            if terminal.is_some() {
                return Ok(terminal);
            }
        }
        Ok(terminal)
    }
    pub(super) async fn send(&self, notification: wire::AgentNotification) -> Result<(), Error> {
        match self {
            Self::Native { .. } => {
                Err(Error::internal_error().data("native delivery requires original event"))
            }
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
