//! Official ACP values dispatched directly; no JSON-RPC envelopes or SDK queues.
//! The owner must await `close` before discarding the dispatcher.
use super::*;
use pablo_core::{
    children::{
        AgentRef,
        ledger::{
            RootLedger,
            resources::{ResourceLease, Resources},
        },
    },
    deployment::PreparedRun,
};

pub(super) struct AdmittedChild {
    pub prepared: PreparedRun,
    pub accounting: worker::AccountingScope,
    pub root_cancellation: CancellationToken,
    pub parent: opentelemetry::Context,
    pub lease: Option<Arc<ResourceLease>>,
}

pub(super) struct Dispatcher {
    state: Arc<Mutex<State>>,
    options: Arc<Options>,
    cancellation: CancellationToken,
    delivery: delivery::Delivery,
    closing: tokio::sync::Mutex<CloseState>,
}
#[derive(Default)]
struct CloseState {
    task: Option<tokio::task::JoinHandle<Result<(), String>>>,
    result: Option<Result<(), String>>,
}
impl Dispatcher {
    fn ensure_open(&self) -> Result<(), Error> {
        if self.cancellation.is_cancelled() {
            Err(Error::internal_error().data("typed ACP connection closed"))
        } else {
            if self
                .state
                .lock()
                .unwrap()
                .admitted_child
                .as_ref()
                .is_some_and(|child| child.root_cancellation.is_cancelled())
            {
                return Err(Error::internal_error().data("root cancelled"));
            }
            Ok(())
        }
    }

    pub fn new(
        options: impl Into<Arc<Options>>,
    ) -> (Self, async_channel::Receiver<delivery::TypedUpdate>) {
        let (tx, rx) = async_channel::bounded(1);
        let cancellation = CancellationToken::new();
        (
            Self {
                state: Arc::new(Mutex::new(State::default())),
                options: options.into(),
                cancellation: cancellation.clone(),
                closing: tokio::sync::Mutex::new(CloseState::default()),
                delivery: delivery::Delivery::Typed {
                    sender: tx,
                    closed: cancellation,
                },
            },
            rx,
        )
    }
    /// Internal C3.22 execution path. Authority was fixed by prepare_child;
    /// registration and the active slot commit together before any task starts.
    /// The supervisor still owns aggregate event/process/context admission.
    pub fn new_child(
        options: Options,
        prepared: PreparedRun,
        ledger: RootLedger,
        agent: &AgentRef,
        root_cancellation: CancellationToken,
        parent: opentelemetry::Context,
    ) -> Result<(Self, async_channel::Receiver<delivery::TypedUpdate>), String> {
        if !prepared.is_child() || options.deployment.is_none() || root_cancellation.is_cancelled()
        {
            return Err("invalid child admission".into());
        }
        let lease = ledger
            .admit_child(
                agent,
                prepared.spec().limits.clone(),
                Resources {
                    active_children: 1,
                    ..Resources::default()
                },
            )
            .map_err(|_| "child admission rejected")?;
        Self::bind_child(
            Arc::new(options),
            prepared,
            ledger,
            agent,
            root_cancellation,
            parent,
            Arc::new(lease),
        )
    }
    /// Bind an already admitted/promoted child without registering it twice.
    pub fn bind_child(
        options: Arc<Options>,
        prepared: PreparedRun,
        ledger: RootLedger,
        agent: &AgentRef,
        root_cancellation: CancellationToken,
        parent: opentelemetry::Context,
        lease: Arc<ResourceLease>,
    ) -> Result<(Self, async_channel::Receiver<delivery::TypedUpdate>), String> {
        if !prepared.is_child()
            || options.deployment.is_none()
            || !lease.is_active_child(&ledger, agent.agent_id())
        {
            return Err("invalid child admission".into());
        }
        let (dispatcher, updates) = Self::new(options);
        dispatcher.state.lock().unwrap().admitted_child = Some(AdmittedChild {
            prepared,
            accounting: worker::AccountingScope {
                ledger,
                agent_id: agent.agent_id().into(),
            },
            root_cancellation,
            parent,
            lease: Some(lease),
        });
        Ok((dispatcher, updates))
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
    pub async fn prompt_observed(
        &self,
        request: wire::PromptRequest,
        request_cancel: &CancellationToken,
        receipt: tokio::sync::oneshot::Sender<handlers::Completion>,
    ) -> Result<wire::PromptResponse, Error> {
        self.ensure_open()?;
        handlers::start_prompt(
            self.state.clone(),
            self.options.clone(),
            &self.cancellation,
            request,
        )?
        .finish_observed(&self.delivery, request_cancel.cancelled(), Some(receipt))
        .await
    }
    pub async fn close(&self) -> Result<(), String> {
        let mut closing = self.closing.lock().await;
        if let Some(result) = &closing.result {
            return result.clone();
        }
        self.cancellation.cancel();
        if let delivery::Delivery::Typed { sender, .. } = &self.delivery {
            sender.close();
        }
        {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            if let Some(cancel) = &state.cancellation {
                cancel.cancel();
            }
            if let Some(events) = state.events.take() {
                events.close();
            }
        }
        if closing.task.is_none() {
            let worker = self.state.lock().unwrap().worker.take();
            if let Some(worker) = worker {
                closing.task = Some(tokio::spawn(async move { worker.shutdown().await }));
            }
        }
        // Retain the join handle across cancellation of an individual close
        // caller. A later caller must await the same shutdown, never skip it.
        let result = if let Some(task) = &mut closing.task {
            match task.await {
                Ok(result) => result,
                Err(_) => Err("ACP runtime cleanup failed".into()),
            }
        } else {
            Ok(())
        };
        closing.task = None;
        closing.result = Some(result.clone());
        self.state.lock().unwrap().admitted_child.take();
        result
    }
}
impl Drop for Dispatcher {
    fn drop(&mut self) {
        self.cancellation.cancel();
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        if let Some(cancel) = &state.cancellation {
            cancel.cancel();
        }
        if let Some(events) = state.events.as_ref() {
            events.close();
        }
    }
}

#[cfg(test)]
mod tests;
