//! Official ACP values dispatched directly; no JSON-RPC envelopes or SDK queues.
//! The owner must await `close` before discarding the dispatcher.
use super::*;

pub(super) struct Dispatcher {
    state: Arc<Mutex<State>>,
    options: Arc<Options>,
    cancellation: CancellationToken,
    delivery: delivery::Delivery,
}
impl Dispatcher {
    fn ensure_open(&self) -> Result<(), Error> {
        if self.cancellation.is_cancelled() {
            Err(Error::internal_error().data("typed ACP connection closed"))
        } else {
            Ok(())
        }
    }

    pub fn new(options: Options) -> (Self, async_channel::Receiver<delivery::TypedUpdate>) {
        let (tx, rx) = async_channel::bounded(1);
        let cancellation = CancellationToken::new();
        (
            Self {
                state: Arc::new(Mutex::new(State::default())),
                options: Arc::new(options),
                cancellation: cancellation.clone(),
                delivery: delivery::Delivery::Typed {
                    sender: tx,
                    closed: cancellation,
                },
            },
            rx,
        )
    }
    pub fn initialize(
        &self,
        request: wire::InitializeRequest,
    ) -> Result<wire::InitializeResponse, Error> {
        self.ensure_open()?;
        handlers::initialize(&self.state, request)
    }
    pub fn new_session(
        &self,
        request: wire::NewSessionRequest,
    ) -> Result<wire::NewSessionResponse, Error> {
        self.ensure_open()?;
        handlers::new_session(&self.state, &self.options, request)
    }
    pub fn cancel(&self, request: wire::CancelNotification) -> Result<(), Error> {
        handlers::cancel(&self.state, request)
    }
    pub async fn prompt(
        &self,
        request: wire::PromptRequest,
        request_cancel: &CancellationToken,
    ) -> Result<wire::PromptResponse, Error> {
        self.ensure_open()?;
        handlers::start_prompt(
            self.state.clone(),
            self.options.clone(),
            &self.cancellation,
            request,
        )?
        .finish(&self.delivery, request_cancel.cancelled())
        .await
    }
    pub async fn close(&self) -> Result<(), String> {
        self.cancellation.cancel();
        if let delivery::Delivery::Typed { sender, .. } = &self.delivery {
            sender.close();
        }
        if let Some(events) = self.state.lock().unwrap().events.take() {
            events.close();
        }
        let worker = self.state.lock().unwrap().worker.take();
        if let Some(worker) = worker {
            worker
                .shutdown()
                .await
                .map_err(|_| "ACP runtime cleanup failed")?;
        }
        Ok(())
    }
}
impl Drop for Dispatcher {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(events) = self.state.lock().unwrap().events.as_ref() {
            events.close();
        }
    }
}

#[cfg(test)]
mod tests;
