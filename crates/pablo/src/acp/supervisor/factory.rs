//! Root ownership is admitted before opening any MCP capability. The caller
//! retains the factory across setup and awaits setup/close instead of dropping it.
use super::*;

pub(crate) struct RootFactory {
    pub root: AgentRef,
    pub ledger: RootLedger,
    options: Arc<Options>,
    parent: PreparedRun,
}
impl RootFactory {
    pub fn configured(
        options: &Options,
        parent: Option<&PreparedRun>,
    ) -> Result<Option<Self>, String> {
        parent
            .filter(|p| !p.is_child() && p.deployment().options()["children"]["enabled"] == true)
            .map(|p| Self::new(Arc::new(options.clone()), p.clone()).map_err(str::to_owned))
            .transpose()
    }

    /// Setup and execution remain polled through cancellation. The consumer runs
    /// alongside the root while Runtime joins its owner before terminal emission.
    pub async fn run<S: pablo_core::EventSink, T: opentelemetry::trace::Tracer + Send + Sync>(
        self,
        runtime: pablo_core::Runtime<T>,
        provider: &dyn pablo_core::Provider,
        cancellation: &CancellationToken,
        sink: &mut S,
    ) -> Result<Result<RunOutcome, pablo_core::RunError>, String>
    where
        T::Span: Send + Sync + 'static,
    {
        let spec = self.parent.spec().clone();
        let mut sink = pablo_core::events::tree::TreeSink::new(
            |event: &RunEvent| sink.emit(event),
            &self.ledger,
            self.root.clone(),
        )
        .map_err(|e| e.to_string())?;
        let (owner, updates) = self.open(cancellation).await?;
        let runtime = match runtime.with_root_owner(owner.clone()) {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = owner.close().await;
                return Err(error.to_string());
            }
        };
        let consume = Supervisor::consume_updates(updates, sink.clone(), cancellation.clone());
        let run = async {
            let result = runtime
                .run_with_tools(
                    &spec,
                    provider,
                    &owner.inner.parent_tools,
                    cancellation,
                    &mut sink,
                )
                .await;
            // Also closes the consumer on rejection before runtime claims ownership.
            let cleanup = owner.close().await;
            (result, cleanup)
        };
        let ((result, cleanup), delivery) = tokio::join!(run, consume);
        if delivery.is_err() || cleanup.is_err() {
            return Err("root tree delivery or cleanup failed".into());
        }
        Ok(result)
    }

    pub fn new(options: Arc<Options>, parent: PreparedRun) -> Result<Self, &'static str> {
        if parent.is_child() || options.deployment.is_none() {
            return Err("invalid root factory");
        }
        let session = parent
            .spec()
            .session_id
            .clone()
            .ok_or("root session missing")?;
        let root = AgentRef::root(uuid::Uuid::new_v4().to_string(), session);
        let ledger = RootLedger::new(&root, parent.spec().limits.clone())
            .map_err(|_| "invalid root limits")?;
        if parent.trace_path().is_some() {
            ledger
                .configure_trace(&parent.spec().trace)
                .map_err(|_| "invalid root trace capacity")?;
        }
        Ok(Self {
            root,
            ledger,
            options,
            parent,
        })
    }
    /// Permit admission precedes credential resolution, MCP initialize/list and
    /// Skill/MCP setup. The returned supervisor retains it until owned cleanup.
    pub(super) async fn open(
        self,
        cancellation: &CancellationToken,
    ) -> Result<(Arc<Supervisor>, async_channel::Receiver<Update>), String> {
        if cancellation.is_cancelled() || Instant::now() >= self.ledger.deadline() {
            return Err("root setup cancelled or expired".into());
        }
        let required = self.parent.mcp_resources().map_err(|e| e.to_string())?;
        let lease = if required.mcp_sessions == 0 {
            None
        } else {
            Some(Arc::new(
                self.ledger
                    .reserve_resources(self.root.agent_id(), required)
                    .map_err(|_| "root MCP admission rejected")?,
            ))
        };
        let tools = Arc::new(if self.parent.needs_async_tools() {
            self.options
                .deployment
                .as_ref()
                .unwrap()
                .tools(&self.parent, self.ledger.deadline(), cancellation)
                .await?
        } else {
            self.parent.tools().map_err(|e| e.to_string())?
        });
        // A failed factory has already joined its partial setup. Once setup
        // succeeds, this owner also covers a late cancellation/constructor error.
        if cancellation.is_cancelled() || Instant::now() >= self.ledger.deadline() {
            let _ = tools.close().await;
            return Err("root setup cancelled or expired".into());
        }
        match Supervisor::with_root_resources(
            self.options,
            self.root,
            self.ledger,
            cancellation,
            self.parent,
            tools.clone(),
            lease.clone(),
        ) {
            Ok((supervisor, updates)) => Ok((Arc::new(supervisor), updates)),
            Err(error) => {
                let _ = tools.close().await;
                Err(error.into())
            }
        }
    }
}
