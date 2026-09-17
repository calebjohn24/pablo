//! Immutable provider resources. The runtime owns all attempt selection.
use super::*;
use crate::deployment::ResolvedRoute;

pub struct ProviderRoute {
    pub(crate) resolved: ResolvedRoute,
    pub(crate) providers: Vec<Box<dyn Provider>>,
    pub(crate) router: Option<crate::gateway::JevProvider>,
}
impl ProviderRoute {
    /// Hosts bind one adapter to each previously authorized entry, in order.
    /// This constructor never performs model requests or reads credentials.
    pub fn new(
        resolved: ResolvedRoute,
        providers: Vec<Box<dyn Provider>>,
    ) -> Result<Self, &'static str> {
        Self::build(resolved, providers, None)
    }
    pub fn with_router(
        resolved: ResolvedRoute,
        providers: Vec<Box<dyn Provider>>,
        router: crate::gateway::JevProvider,
    ) -> Result<Self, &'static str> {
        Self::build(resolved, providers, Some(router))
    }
    fn build(
        resolved: ResolvedRoute,
        providers: Vec<Box<dyn Provider>>,
        router: Option<crate::gateway::JevProvider>,
    ) -> Result<Self, &'static str> {
        if resolved.router() != router.as_ref().map(|r| &r.config) {
            return Err("route requires its configured Jev classifier");
        }
        if providers.len() != resolved.entries().len()
            || providers.iter().any(|p| p.route().is_some())
        {
            return Err("route requires one non-routing adapter per entry");
        }
        for (entry, provider) in resolved.entries().iter().zip(&providers) {
            if provider.name() != entry.profile().provider.name() {
                return Err("route adapter provider identity mismatch");
            }
            provider.validate_model(&entry.profile().model, entry.max_output_tokens())?;
            provider.validate_reasoning(entry.profile().reasoning, entry.max_output_tokens())?;
        }
        Ok(Self {
            resolved,
            providers,
            router,
        })
    }
}
impl Provider for ProviderRoute {
    fn route(&self) -> Option<&ProviderRoute> {
        Some(self)
    }
    fn name(&self) -> &'static str {
        self.providers[0].name()
    }
    fn stream<'a>(
        &'a self,
        _request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        // A route cannot hide multiple dispatches behind the provider boundary.
        Box::pin(async {
            Err(ProviderError {
                code: FailureCode::UnsupportedProviderContent,
                delivery: DeliveryCertainty::NotSent,
                retry_class: None,
            })
        })
    }
}
