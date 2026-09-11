//! Root ownership is admitted before opening any MCP capability. The caller
//! retains the factory across setup and awaits setup/close instead of dropping it.
use super::*;

pub(super) struct RootFactory {
    pub root: AgentRef,
    pub ledger: RootLedger,
    options: Arc<Options>,
    parent: PreparedRun,
}
impl RootFactory {
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
    pub async fn open(
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
