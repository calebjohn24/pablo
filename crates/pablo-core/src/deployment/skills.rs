use super::*;
use std::path::Path;

impl ResolvedDeployment {
    /// Resolve explicit policy-visible Skill roots without scanning or reading credentials.
    pub fn skill_roots(
        &self,
        workspace: Option<&Path>,
    ) -> Result<Vec<crate::skills::Root>, ConfigError> {
        if self.options()["skills"]["roots"]
            .as_object()
            .is_some_and(|roots| roots.is_empty())
        {
            return Ok(Vec::new());
        }
        let mut request = ResolveRequest::new(self.config_root.clone(), "unused");
        request.path_bindings = self.path_bindings.clone();
        let options = self.options();
        let physical = |value: &Value| {
            if value["base"] == "workspace"
                && let Some(workspace) = workspace
            {
                Ok(workspace.join(input::relative(value["path"].as_str().unwrap())?))
            } else {
                validate::physical(value, options, &request, true)
            }
        };
        let mut roots = Vec::new();
        for (id, value) in options["skills"]["roots"]
            .as_object()
            .ok_or_else(|| error("config_invalid_value", "/options/skills/roots"))?
        {
            let path = physical(value)?;
            for layer in self.config["authority"].as_array().unwrap() {
                if let Some(allowed) = layer.get("skill_roots").and_then(Value::as_array) {
                    let admitted = allowed
                        .iter()
                        .map(physical)
                        .collect::<Result<Vec<_>, _>>()?
                        .iter()
                        .any(|root| path.starts_with(root));
                    if !admitted {
                        let mut e = error("config_authority_violation", "/options/skills/roots");
                        e.authority_id = layer["id"].as_str().map(Into::into);
                        return Err(e);
                    }
                }
            }
            roots.push(crate::skills::Root {
                id: id.clone(),
                path,
            });
        }
        Ok(roots)
    }
}
