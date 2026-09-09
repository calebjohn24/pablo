use std::{collections::VecDeque, time::Duration};

use futures_util::{
    future::BoxFuture,
    stream::{self, BoxStream},
};
use opentelemetry::Context;
use tokio::time::Instant;

use crate::{DeliveryCertainty, FailureCode, FinishReason, Message, Usage, tool::ToolDescriptor};
use tokio_util::sync::CancellationToken;

/// No credentials are serialized or copied into a request. Adapters own them.
pub struct ModelRequest<'a> {
    pub model: &'a str,
    pub input: &'a str,
    pub instructions: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolDescriptor],
    /// Ask for a final answer when no further tool result can be consumed.
    /// The runtime still enforces its limits if a provider ignores this hint.
    pub allow_tool_calls: bool,
    pub max_output_tokens: u32,
    pub deadline: Instant,
    pub context: Context,
    pub cancellation: CancellationToken,
}

#[derive(Clone, Debug)]
pub enum ProviderEvent {
    /// Trusted adapter-reported actual charge, once per call before Finished.
    Cost {
        microusd: u64,
    },
    TextDelta(String),
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        id: String,
        delta: String,
    },
    /// Exactly one finish followed by EOF. Usage must be assembled before this.
    Finished {
        reason: FinishReason,
        usage: Usage,
    },
}

/// Closed codes keep raw transport errors, request bodies, and keys out of traces.
#[derive(Clone, Copy, Debug)]
pub struct ProviderError {
    pub code: FailureCode,
    pub delivery: DeliveryCertainty,
}

pub type ProviderStream<'a> = BoxStream<'a, Result<ProviderEvent, ProviderError>>;

/// Enforced per-call ceilings attested by an adapter, never inferred prices.
#[derive(Clone, Copy, Debug, Default)]
pub struct AccountingBounds {
    pub tokens: Option<u64>,
    pub cost_microusd: Option<u64>,
}

/// Futures and streams must yield promptly; dropping them cancels owned work.
/// Adapters must bound frame allocation before creating normalized deltas.
pub trait Provider: Send + Sync {
    /// An adapter attests enforcement for this model/request shape, including
    /// all delivery paths. Observed usage or estimated pricing is insufficient.
    /// The default deliberately cannot support hard aggregate ceilings.
    fn accounting_bounds(&self, _model: &str, _max_output_tokens: u32) -> AccountingBounds {
        AccountingBounds::default()
    }

    fn name(&self) -> &'static str;
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>>;
}

/// Offline fixture using the same asynchronous boundary as future HTTP adapters.
/// Each item is yielded separately; optional delays make streaming observable.
pub struct ScriptedProvider {
    items: Vec<(Duration, Result<ProviderEvent, ProviderError>)>,
}

impl ScriptedProvider {
    pub fn new(items: Vec<(Duration, Result<ProviderEvent, ProviderError>)>) -> Self {
        Self { items }
    }

    pub fn text(chunks: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut items: Vec<_> = chunks
            .into_iter()
            .map(|text| (Duration::ZERO, Ok(ProviderEvent::TextDelta(text.into()))))
            .collect();
        items.push((
            Duration::ZERO,
            Ok(ProviderEvent::Finished {
                reason: FinishReason::Stop,
                usage: Usage::default(),
            }),
        ));
        Self::new(items)
    }
}

impl Provider for ScriptedProvider {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn stream<'a>(
        &'a self,
        _request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            let items = VecDeque::from(self.items.clone());
            let stream = stream::unfold(items, |mut items| async move {
                let (delay, event) = items.pop_front()?;
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                Some((event, items))
            });
            Ok(Box::pin(stream) as ProviderStream<'a>)
        })
    }
}
