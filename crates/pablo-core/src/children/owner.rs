//! Root runtime ownership boundary. Implementations own their child execution;
//! close must join it and may not treat cancellation as permission to detach.
use super::{AgentRef, ledger::RootLedger};
use futures_util::future::BoxFuture;

pub trait RootOwner: crate::Tool {
    fn root(&self) -> &AgentRef;
    fn ledger(&self) -> &RootLedger;
    fn cancellation(&self) -> &crate::CancellationToken;
    /// The model tool must also intersect the admitted parent's host ceilings.
    fn model_tool_allowed(&self) -> bool;
    /// One runtime may claim this temporary root, including invalid/preflight runs.
    fn claim_root(&self) -> bool;
    /// Stop admission and signal children without cancelling the parent's token.
    fn cancel(&self);
    fn close(&self) -> BoxFuture<'_, Result<(), &'static str>>;
}
