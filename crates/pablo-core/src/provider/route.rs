//! Immutable provider resources. The runtime owns all attempt selection.
use super::*;
use crate::deployment::ResolvedRoute;

pub struct ProviderRoute {
    pub(crate) resolved: ResolvedRoute,
    pub(crate) providers: Vec<Box<dyn Provider>>,
}
impl ProviderRoute {
    /// Hosts bind one adapter to each previously authorized entry, in order.
    /// This constructor never performs model requests or reads credentials.
    pub fn new(
        resolved: ResolvedRoute,
        providers: Vec<Box<dyn Provider>>,
    ) -> Result<Self, &'static str> {
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
        }
        Ok(Self {
            resolved,
            providers,
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
