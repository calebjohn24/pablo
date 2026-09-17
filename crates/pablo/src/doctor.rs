//! Local diagnostic projection. Probes are explicit and never execute an agent.
use crate::config;
use pablo_core::{
    CancellationToken,
    deployment::CredentialConsumer,
    gateway::{GatewayProvider, ProbeFailure},
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Write},
    process::ExitCode,
    time::{Duration, Instant},
};

pub const HELP: &str = "\nDiagnostics: pablo doctor [--json] [RUN CONFIGURATION OPTIONS]\n  Offline by default. --probe provider permits one small model request;\n  --probe mcp permits configured startup/negotiation and joined cleanup.\n";

pub fn setup_hint(message: &str) -> Option<&'static str> {
    if message.contains("credential")
        || message.contains("API_KEY")
        || message.contains("environment file")
        || message.contains(".env")
    {
        Some(
            "Fix: set a valid credential at its declared source; run pablo doctor with the same configuration to check presence.",
        )
    } else if message.contains("mcp") {
        Some(
            "Fix: check the MCP executable/endpoint and credentials; run pablo doctor --probe mcp with the same configuration.",
        )
    } else if message.contains("authority")
        || message.contains("policy")
        || message.contains("override_forbidden")
    {
        Some(
            "Fix: inspect the owning static policy with pablo doctor and config explain; request only host-permitted capabilities.",
        )
    } else if message.contains("model") || message.contains("provider") {
        Some(
            "Fix: check the provider/model configuration with pablo doctor; use --probe provider to check remote acceptance.",
        )
    } else {
        None
    }
}

fn finding(report: &mut Value, code: u8, component: &str, cause: &str, fix: &str) {
    report["findings"]
        .as_array_mut()
        .unwrap()
        .push(json!({"exit_code":code,"component":component,"cause":cause,"fix":fix}));
}
fn configuration_error(report: &mut Value, error: &str) {
    let (code, cause, fix) = if error.starts_with("config_credential") {
        (
            3,
            "A declared credential is missing or unusable.",
            "Set the credential at its declared source; then run pablo doctor again.",
        )
    } else if error.starts_with("config_authority")
        || error.starts_with("config_override_forbidden")
    {
        (
            7,
            "Static policy or deployment authority denies this configuration.",
            "Review config explain and the owning policy; change only settings permitted by the host.",
        )
    } else if error.contains("unsupported") || error.contains("model") {
        (
            5,
            "The selected model/profile is unsupported or invalid.",
            "Select a supported provider/model profile; use --probe provider to check remote acceptance.",
        )
    } else {
        (
            2,
            "Configuration or local paths could not be admitted.",
            "Run pablo config validate/explain with the same bootstrap options and correct the reported source.",
        )
    };
    finding(report, code, "configuration", cause, fix);
}
fn presence<T>(report: &mut Value, component: &str, result: Result<Option<T>, String>) -> Value {
    match result {
        Ok(Some(_)) => json!("present; not remotely verified"),
        Ok(None) => json!("not configured"),
        Err(_) => {
            finding(
                report,
                3,
                component,
                "Credential is missing or unusable.",
                "Supply a valid credential through the declared source; do not place it in command arguments.",
            );
            json!("missing or unusable")
        }
    }
}
fn host(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|s| reqwest::Url::parse(s).ok())
        .and_then(|u| u.host_str().map(str::to_owned))
        .map_or(Value::Null, Value::String)
}
fn settings(options: &Value) -> Value {
    let servers: Vec<_> = options["mcp"]["servers"].as_object().into_iter().flatten().map(|(name, server)| json!({"name":name,"transport":server["transport"],"required":server["required"],"endpoint_host":host(&server["url"]),"credential_bindings":server["env"].as_object().or_else(||server["headers"].as_object()).map_or(0,|m|m.len()),"connectivity":"not probed"})).collect();
    let remotes: Vec<_> = options["a2a"]["remotes"].as_object().into_iter().flatten().map(|(name, remote)| json!({"name":name,"endpoint_host":host(&remote["endpoint"]),"card_host":host(&remote["card_url"]),"connectivity":"not probed"})).collect();
    json!({"policy":options["policy"],"shell":{"enabled":options["shell"]["enabled"],"executable":"/bin/sh","available":std::path::Path::new("/bin/sh").is_file(),"environment_binding_count":options["shell"]["environment"]["values"].as_object().map_or(0,|m|m.len())},"filesystem":options["filesystem"],"skills":{"roots":options["skills"]["roots"],"activation":options["skills"]["activate"]},"mcp":servers,"a2a":remotes,"children":options["children"],"limits":options["limits"],"trace":{"path":options["trace"]["path"],"capture_content":options["trace"]["capture_content"],"max_bytes":options["trace"]["max_bytes"]},"otel":{"sdk_disabled":options["otel"]["sdk_disabled"],"exporter":options["otel"]["exporter"],"endpoint_host":host(&options["otel"]["endpoint"]),"protocol":options["otel"]["protocol"],"sampler":options["otel"]["sampler"],"propagators":options["otel"]["propagators"],"service_name":options["otel"]["service_name"],"resource_attribute_names":options["otel"]["resource_attributes"].as_object().map(|m|m.keys().collect::<Vec<_>>()),"export":"not started"}})
}
async fn provider_probe(
    report: &mut Value,
    provider: &GatewayProvider,
    model: &str,
    cancellation: &CancellationToken,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return,
        result = tokio::time::timeout_at(deadline, provider.probe(model, deadline)) => result,
    };
    match result {
        Ok(Ok(())) => {
            report["probe_result"] =
                json!("provider accepted HTTP/SSE request; output not evaluated");
        }
        result => {
            let failure = result
                .ok()
                .and_then(Result::err)
                .unwrap_or(ProbeFailure::Transport);
            let (code, cause, fix) = match failure {
                ProbeFailure::Authentication => (
                    4,
                    "Provider rejected authentication.",
                    "Replace or enable the credential at its declared source; check account access.",
                ),
                ProbeFailure::ModelRequest => (
                    5,
                    "Provider rejected the selected model request or endpoint.",
                    "Check the model identifier, endpoint and capability profile in config explain.",
                ),
                ProbeFailure::Transport => (
                    8,
                    "Provider could not be reached within the probe deadline.",
                    "Check connectivity and the configured endpoint, then retry the explicit probe.",
                ),
                ProbeFailure::Response => (
                    8,
                    "Provider did not accept the expected streaming request.",
                    "Check service availability and the endpoint/profile; raw response bodies are intentionally omitted.",
                ),
            };
            finding(report, code, "provider", cause, fix);
        }
    }
}
async fn diagnose(
    args: Vec<OsString>,
    probe: Option<&str>,
    report: &mut Value,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    let options = config::Options::parse(
        "run".into(),
        std::iter::once("doctor diagnostic".into()).chain(args),
    )?;
    if let Some(resolved) = options.configured()? {
        report["configuration"] = json!({"mode":"deployment","fingerprint":resolved.fingerprint(),"sources":resolved.sources(),"provenance":resolved.provenance().iter().filter(|(key,_)| ["model", "models", "routes", "credentials", "policy", "shell", "skills", "mcp", "a2a", "children", "trace", "otel"].iter().any(|part| key.split(['/', '.']).any(|value| value==*part))).collect::<std::collections::BTreeMap<_,_>>()});
        report["settings"] = settings(resolved.options());
        report["settings"]["authority"] = resolved.config()["authority"].clone();
        let profiles = resolved.model_route().map(|r|r.entries().iter().map(|e|json!({"name":e.name(),"provider":e.profile().provider.name(),"model":e.profile().model,"endpoint_host":host(&json!(e.profile().endpoint))})).collect::<Vec<_>>()).unwrap_or_else(||vec![json!({"provider":resolved.options()["model"]["provider"],"model":resolved.options()["model"]["id"],"endpoint_host":host(&resolved.options()["model"]["endpoint"])})]);
        report["models"] = json!(profiles);
        let prepared = resolved
            .prepare_run(pablo_core::deployment::RunInput {
                input: "doctor diagnostic".into(),
                workspace: None,
                session_id: Some("doctor-inspection".into()),
            })
            .map_err(|e| e.to_string())?;
        report["settings"]["trace"]["path_checks"] =
            json!("parent and exclusive target checked; no trace created");
        let roots = resolved.skill_roots(None).map_err(|e| e.to_string())?;
        let catalog = pablo_core::skills::discover(
            &roots,
            &CancellationToken::new(),
            Instant::now() + Duration::from_millis(500),
        )
        .map_err(|e| e.to_string())?;
        report["settings"]["skills"]["diagnostics"] = serde_json::to_value(&catalog.diagnostics)
            .map_err(|_| "cannot encode Skill diagnostics")?;
        if !catalog.diagnostics.is_empty() {
            finding(
                report,
                2,
                "skills",
                "One or more Skill roots or metadata entries have diagnostics.",
                "Use pablo skills list with the same config and correct the indicated root or metadata.",
            );
        }
        let profile = prepared.model_profile().map_err(|e| e.to_string())?;
        let mut selected_secret = None;
        let count = prepared.model_route().map_or(1, |r| r.entries().len());
        for index in 0..count {
            let secret = if prepared.model_route().is_some() {
                prepared
                    .route_credential(index, &pablo_core::deployment::ProcessCredentials)
                    .map(Some)
            } else {
                prepared.credential(
                    profile.provider.credential_consumer(),
                    &pablo_core::deployment::ProcessCredentials,
                )
            };
            report["models"][index]["credential"] = match secret {
                Ok(value) => {
                    let state = if value.is_some() {
                        json!("present; not remotely verified")
                    } else {
                        json!("not configured")
                    };
                    if index == 0 {
                        selected_secret = value;
                    }
                    state
                }
                Err(_) => presence::<()>(report, "provider", Err("credential".into())),
            };
        }
        if let Some(router) = prepared.model_route().and_then(|route| route.router()) {
            let credential = presence(
                report,
                "router",
                prepared
                    .router_credential(&pablo_core::deployment::ProcessCredentials)
                    .map_err(|e| e.to_string()),
            );
            report["models"].as_array_mut().unwrap().push(json!({
                "name": "task_classifier", "provider": "vercel", "model": router.model,
                "endpoint_host": "ai-gateway.vercel.sh", "credential": credential,
                "purpose": "task_start_classification"
            }));
        }
        let otel = prepared.credential(
            CredentialConsumer::OtelHeaders,
            &pablo_core::deployment::ProcessCredentials,
        );
        if let Ok(headers) = &otel
            && let Err(error) =
                crate::otel::Telemetry::check_configured(&prepared, headers.as_ref())
        {
            configuration_error(report, &error);
        }
        report["settings"]["otel"]["credential"] =
            presence(report, "otel", otel.map_err(|e| e.to_string()));
        for (index, (name, _)) in resolved
            .mcp()
            .map_err(|e| e.to_string())?
            .servers
            .iter()
            .enumerate()
        {
            let status = prepared
                .mcp_credential_presence(name, &pablo_core::deployment::ProcessCredentials)
                .map(|count| (count > 0).then_some(count))
                .map_err(|e| e.to_string());
            report["settings"]["mcp"][index]["credentials"] =
                presence(report, &format!("mcp/{name}"), status);
        }
        for (index, (name, _)) in resolved
            .a2a()
            .map_err(|e| e.to_string())?
            .remotes
            .iter()
            .enumerate()
        {
            let status = prepared
                .a2a_credential(name, &pablo_core::deployment::ProcessCredentials)
                .map_err(|e| e.to_string());
            report["settings"]["a2a"][index]["credential"] =
                presence(report, &format!("a2a/{name}"), status);
        }
        if probe == Some("provider") {
            let bootstrap = options.deployment.as_ref().unwrap();
            let provider = if let Some(endpoint) = &bootstrap.fixture_endpoint {
                Some(
                    GatewayProvider::configured_fixture(&profile, endpoint)
                        .map_err(str::to_owned)?,
                )
            } else if let Some(secret) = selected_secret {
                Some(
                    GatewayProvider::configured(
                        &profile,
                        secret
                            .expose_for(profile.provider.credential_consumer(), &profile.endpoint)
                            .map_err(|e| e.to_string())?,
                    )
                    .map_err(str::to_owned)?,
                )
            } else {
                None
            };
            if let Some(provider) = provider {
                provider_probe(report, &provider, &profile.model, cancellation).await;
            }
        }
        if probe == Some("mcp") {
            // The setup API owns its deadline and joins partial startup failures.
            match options
                .deployment
                .as_ref()
                .unwrap()
                .tools(
                    &prepared,
                    tokio::time::Instant::now() + Duration::from_secs(10),
                    cancellation,
                )
                .await
            {
                Ok(tools) => {
                    let omitted = !tools.mcp_omissions().is_empty();
                    if tools.close().await.is_ok() && !omitted {
                        report["probe_result"] =
                            json!("MCP negotiation completed; owned clients closed");
                    } else {
                        finding(
                            report,
                            6,
                            "mcp",
                            "An MCP server failed startup, negotiation or cleanup.",
                            "Check server shutdown behavior and its configured timeouts.",
                        );
                    }
                }
                Err(error)
                    if cancellation.is_cancelled() && !error.starts_with("config_mcp_cleanup") => {}
                Err(_) => finding(
                    report,
                    6,
                    "mcp",
                    "MCP startup or negotiation failed.",
                    "Check the configured executable/endpoint, credential sources and protocol support; use config explain for provenance.",
                ),
            }
        }
    } else {
        let spec = options.spec()?;
        let kind = options.provider.unwrap_or_default();
        let mut defaults: Value = serde_json::from_str(pablo_core::deployment::DEFAULT_OPTIONS)
            .map_err(|_| "invalid built-in defaults")?;
        defaults["shell"]["enabled"] = json!(!options.no_shell);
        defaults["filesystem"] =
            json!({"enabled":!options.no_filesystem,"write":options.allow_write});
        defaults["limits"] =
            serde_json::to_value(&spec.limits).map_err(|_| "cannot encode limits")?;
        defaults["policy"] =
            serde_json::to_value(options.policy()?).map_err(|_| "cannot encode policy")?;
        defaults["trace"]["path"] = json!(options.trace_path);
        defaults["trace"]["capture_content"] = json!(spec.trace.capture_content);
        for (field, names) in [
            ("exporter", vec!["OTEL_TRACES_EXPORTER"]),
            (
                "endpoint",
                vec![
                    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                    "OTEL_EXPORTER_OTLP_ENDPOINT",
                ],
            ),
            (
                "protocol",
                vec![
                    "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
                    "OTEL_EXPORTER_OTLP_PROTOCOL",
                ],
            ),
            ("service_name", vec!["OTEL_SERVICE_NAME"]),
            ("sampler", vec!["OTEL_TRACES_SAMPLER"]),
        ] {
            if let Some(value) = names.iter().find_map(|name| std::env::var(name).ok()) {
                defaults["otel"][field] = json!(value);
            }
        }
        defaults["otel"]["sdk_disabled"] =
            json!(std::env::var("OTEL_SDK_DISABLED").is_ok_and(|v| v.eq_ignore_ascii_case("true")));
        if let Ok(value) = std::env::var("OTEL_PROPAGATORS") {
            defaults["otel"]["propagators"] =
                json!(value.split(',').map(str::trim).collect::<Vec<_>>());
        }
        report["settings"] = settings(&defaults);
        report["settings"]["otel"]["credential"] =
            json!(if std::env::var_os("OTEL_EXPORTER_OTLP_HEADERS").is_some()
                || std::env::var_os("OTEL_EXPORTER_OTLP_TRACES_HEADERS").is_some()
            {
                "present; not exported or remotely verified"
            } else {
                "not configured"
            });
        report["settings"]["otel"]["interpretation"] =
            json!("requested legacy SDK configuration; exporter not constructed");
        let credential_names = if kind == pablo_core::gateway::GatewayKind::Openrouter {
            vec!["OPENROUTER_API_KEY"]
        } else {
            vec!["AI_GATEWAY_API_KEY", "VERCEL_AI_GATEWAY"]
        };
        report["configuration"] = json!({"mode":"legacy run defaults and explicit CLI flags","credential_precedence":"provider environment, then invoking .env or explicit --env-file; no parent search","credential_names":credential_names});
        let key = config::provider_key(kind, options.env_file.as_deref()).and_then(|key| {
            GatewayProvider::selected(kind, &key).map_err(str::to_owned)?;
            Ok(key)
        });
        let state = match &key {
            Ok(_) => json!("present; not remotely verified"),
            Err(_) => presence::<()>(report, "provider", Err("credential".into())),
        };
        report["models"] = json!([{"provider":kind.name(),"model":spec.model,"credential":state,"endpoint_host":host(&json!(kind.endpoint()))}]);
        if probe == Some("provider")
            && let Ok(key) = key
        {
            provider_probe(
                report,
                &GatewayProvider::selected(kind, &key).map_err(str::to_owned)?,
                &spec.model,
                cancellation,
            )
            .await;
        }
        if probe == Some("mcp") {
            report["probe_result"] = json!("no MCP servers configured; use --config for MCP");
        }
    }
    Ok(())
}

fn text_report(report: &Value) -> String {
    let mut lines = vec![
        format!(
            "pablo {} | {}/{} | doctor {:.2} ms",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            report["duration_ms"].as_f64().unwrap_or(0.0)
        ),
        format!(
            "Mode: {}",
            if report["probe"].is_null() {
                "offline; no network or MCP startup"
            } else {
                "explicit probe"
            }
        ),
        format!(
            "Protocols: ACP 1; MCP {}; A2A {}; deployment {}",
            pablo_core::mcp::PROTOCOL_VERSION,
            pablo_core::a2a::PROTOCOL_VERSION,
            pablo_core::deployment::CONTRACT_REVISION
        ),
    ];
    if let Some(models) = report["models"].as_array() {
        for model in models {
            lines.push(format!(
                "Model: {} / {} | credential {}",
                model["provider"].as_str().unwrap_or("unknown"),
                model["model"].as_str().unwrap_or("unknown"),
                model["credential"].as_str().unwrap_or("not checked")
            ));
        }
    }
    lines.push(format!(
        "Configuration: {}",
        report["configuration"]["mode"]
            .as_str()
            .unwrap_or("invalid")
    ));
    if let Some(sources) = report["configuration"]["sources"].as_array() {
        for source in sources {
            lines.push(format!(
                "Source: {} ({})",
                source["locator"].as_str().unwrap_or("unknown"),
                source["id"].as_str().unwrap_or("unknown")
            ));
        }
    }
    for name in [
        "policy",
        "shell",
        "filesystem",
        "skills",
        "mcp",
        "a2a",
        "children",
        "trace",
        "otel",
    ] {
        if !report["settings"][name].is_null() {
            lines.push(format!("{name}: {}", report["settings"][name]));
        }
    }
    lines.push(format!(
        "Probe: {}",
        report["probe_result"].as_str().unwrap_or("not probed")
    ));
    for finding in report["findings"].as_array().unwrap() {
        lines.push(format!(
            "Cause [{} / exit {}]: {}",
            finding["component"].as_str().unwrap_or("configuration"),
            finding["exit_code"],
            finding["cause"].as_str().unwrap_or("unknown")
        ));
        lines.push(format!(
            "Fix: {}",
            finding["fix"]
                .as_str()
                .unwrap_or("Run pablo doctor --help.")
        ));
    }
    lines.push(
        "Details and provenance: add --json; pablo config explain --config PATH shows composition."
            .into(),
    );
    lines
        .join("\n")
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n'
                || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
            {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

pub async fn run(arguments: Vec<OsString>) -> Result<ExitCode, String> {
    let started = Instant::now();
    let mut args = Vec::new();
    let mut probe = None;
    let mut json_output = false;
    let mut input = arguments.into_iter();
    while let Some(arg) = input.next() {
        if arg == "--json" {
            json_output = true;
        } else if arg == "--probe" {
            if probe.is_some() {
                return Err("duplicate --probe; use doctor --help".into());
            }
            probe = Some(
                input
                    .next()
                    .ok_or("--probe requires provider or mcp")?
                    .into_string()
                    .map_err(|_| "invalid probe")?,
            );
        } else if arg == "--help" {
            print!("{HELP}");
            return Ok(ExitCode::SUCCESS);
        } else {
            args.push(arg);
        }
    }
    if probe
        .as_ref()
        .is_some_and(|p| p != "provider" && p != "mcp")
    {
        return Err("--probe requires provider or mcp".into());
    }
    let mut report = json!({"schema_version":1,"binary":{"version":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,"arch":std::env::consts::ARCH},"protocols":{"acp":serde_json::from_str::<Value>(include_str!("../../../docs/acp-lock.json")).map_err(|_|"invalid ACP pin")?,"mcp":pablo_core::mcp::PROTOCOL_VERSION,"mcp_sdk":pablo_core::mcp::SDK_VERSION,"a2a":pablo_core::a2a::PROTOCOL_VERSION,"deployment_contract":pablo_core::deployment::CONTRACT_REVISION,"open_responses":{"profile":pablo_core::gateway::OPEN_RESPONSES_PROFILE,"revision":pablo_core::gateway::OPEN_RESPONSES_REVISION},"genai":pablo_core::telemetry::SEMCONV_REVISION},"probe":probe,"probe_result":"not probed","findings":[]});
    let cancellation = CancellationToken::new();
    let (result, interrupted) = {
        let operation = diagnose(args, probe.as_deref(), &mut report, &cancellation);
        tokio::pin!(operation);
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                cancellation.cancel();
                (operation.await, true)
            },
            result = &mut operation => (result, false),
        }
    };
    if let Err(error) = result {
        configuration_error(&mut report, &error);
    }
    if interrupted {
        finding(
            &mut report,
            130,
            "probe",
            "Diagnosis interrupted; owned cleanup awaited.",
            "Retry the explicit probe when ready.",
        );
    }
    let code = report["findings"]
        .as_array()
        .unwrap()
        .first()
        .and_then(|f| f["exit_code"].as_u64())
        .unwrap_or(0) as u8;
    report["exit_code"] = json!(code);
    report["duration_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
    let encoded = if json_output {
        serde_json::to_string(&report)
    } else {
        Ok(text_report(&report))
    }
    .map_err(|_| "cannot encode doctor report")?;
    writeln!(io::stdout().lock(), "{encoded}").map_err(|_| "cannot write doctor report")?;
    Ok(ExitCode::from(code))
}
