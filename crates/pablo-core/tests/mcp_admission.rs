use pablo_core::{
    PolicyRule,
    mcp::*,
    policy::{Policy as ToolPolicy, PolicySet},
};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
fn settings() -> Settings {
    serde_json::from_value(json!({"servers":{"local":{"transport":"stdio","command":"/usr/bin/printf","args":["safe"],"env":{"MCP_TOKEN":"local-key"}},"remote":{"transport":"http","url":"https://example.test/mcp","headers":{"authorization":"remote-key"},"required":false}}})).unwrap()
}
#[test]
fn pinned_sdk_and_typed_configuration_keep_transport_and_credentials_explicit() {
    assert_eq!(
        serde_json::to_value(rmcp::model::ProtocolVersion::V_2025_11_25).unwrap(),
        PROTOCOL_VERSION
    );
    assert_eq!(
        rmcp::model::ProtocolVersion::LATEST,
        rmcp::model::ProtocolVersion::V_2025_11_25
    );
    let s = settings();
    s.validate().unwrap();
    assert!(s.servers["local"].required());
    assert!(!s.servers["remote"].required());
    assert_eq!(
        s.servers["local"].credentials().collect::<Vec<_>>(),
        vec![("MCP_TOKEN", "local-key", "mcp.env")]
    );
    assert_eq!(
        s.servers["remote"].credentials().collect::<Vec<_>>(),
        vec![("authorization", "remote-key", "mcp.headers")]
    );
    assert!(serde_json::from_value::<Settings>(json!({"servers":{"x":{"transport":"stdio","command":"/bin/true","secret":"not-accepted"}}})).is_err());
    for patch in [
        json!({"transport":"stdio","command":"relative"}),
        json!({"transport":"stdio","command":"/bin/../bin/true"}),
        json!({"transport":"stdio","command":"/bin/true","cwd":{"base":"workspace","path":"../escape"}}),
        json!({"transport":"http","url":"http://example.test/mcp"}),
        json!({"transport":"http","url":"https://user:secret@example.test/mcp"}),
        json!({"transport":"http","url":"https://example.test/mcp?token=x"}),
        json!({"transport":"http","url":"https://example.test/mcp","headers":{"mcp-session-id":"key"}}),
        json!({"transport":"http","url":"https://example.test/mcp","headers":{"Authorization":"key"}}),
        json!({"transport":"stdio","command":"/bin/true","startup_timeout_ms":0}),
    ] {
        let server: Server = serde_json::from_value(patch).unwrap();
        assert!(server.validate().is_err());
    }
    let mut large = s.clone();
    for i in 0..17 {
        large
            .servers
            .insert(format!("server{i}"), s.servers["remote"].clone());
    }
    assert!(large.validate().is_err());
}
#[test]
fn exact_server_tool_and_launcher_denials_intersect_all_authority() {
    let mut s = settings();
    let host = PolicySet::default();
    assert!(s.admit_tool("local", "read", &host).is_ok());
    assert!(s.admit_server("unknown", &host).is_err());
    s.policies=serde_json::from_value(json!([{"servers":{"default":"allow","allow":[{"id":"yes","value":"local"}]}},{"servers":{"default":"allow","deny":[{"id":"no","value":"local"}]}}])).unwrap();
    assert_eq!(
        s.admit_server("local", &host).unwrap_err(),
        PolicyRule::Configured { id: "no".into() }
    );
    s.policies = serde_json::from_value(
        json!([{"tools":{"default":"allow","deny":[{"id":"tools.no","value":"mcp/local/read"}]}}]),
    )
    .unwrap();
    assert!(s.admit_tool("local", "read", &host).is_err());
    assert!(s.admit_tool("local", "Read", &host).is_ok());
    s.policies=serde_json::from_value(json!([{"launchers":{"default":"allow","deny":[{"id":"launch.no","value":"/usr/bin/printf"}]}}])).unwrap();
    assert!(s.admit_server("local", &host).is_err());
    s.policies.clear();
    let tools: ToolPolicy = serde_json::from_value(json!({"tools":{"default":"allow","deny":[{"id":"global.tool.no","value":"mcp/local/read"}]}})).unwrap();
    assert_eq!(
        s.admit_tool("local", "read", &PolicySet::from(tools))
            .unwrap_err(),
        PolicyRule::Configured {
            id: "global.tool.no".into()
        }
    );

    let policy:ToolPolicy=serde_json::from_value(json!({"executables":{"default":"allow","deny":[{"id":"host.no","value":"/usr/bin/printf"}]}})).unwrap();
    assert_eq!(
        s.admit_server("local", &PolicySet::from(policy))
            .unwrap_err(),
        PolicyRule::Configured {
            id: "host.no".into()
        }
    );
}
#[test]
fn acp_requests_cannot_install_launchers_change_arguments_or_supply_credentials() {
    let s = settings();
    let host = PolicySet::default();
    let request = ClientServer {
        name: "local".into(),
        transport: ClientTransport::Stdio {
            command: "/usr/bin/printf".into(),
            args: vec!["safe".into()],
            env: BTreeMap::new(),
        },
    };
    assert_eq!(
        s.admit_client(std::slice::from_ref(&request), &host)
            .unwrap(),
        vec!["local"]
    );
    assert!(
        s.admit_client(&[request.clone(), request.clone()], &host)
            .is_err()
    );
    let mut changed = request.clone();
    changed.name = "workspace-installed".into();
    assert!(s.admit_client(&[changed], &host).is_err());
    for transport in [
        ClientTransport::Stdio {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "touch unexpected".into()],
            env: BTreeMap::new(),
        },
        ClientTransport::Stdio {
            command: "/usr/bin/printf".into(),
            args: vec!["safe".into()],
            env: BTreeMap::from([("MCP_TOKEN".into(), "untrusted-secret".into())]),
        },
        ClientTransport::Http {
            url: "https://different.test/mcp".into(),
            headers: BTreeMap::new(),
        },
    ] {
        assert!(
            s.admit_client(
                &[ClientServer {
                    name: "local".into(),
                    transport
                }],
                &host
            )
            .is_err()
        );
    }
}
#[test]
fn qualified_names_and_provider_aliases_are_bijective_and_collisions_reject() {
    let mut ids = Identities::default();
    let empty = HashSet::new();
    let a = ids.insert("one", "read.file", &empty).unwrap();
    let b = ids.insert("two", "read.file", &empty).unwrap();
    assert_ne!(a, b);
    assert_eq!(a.len(), 52);
    assert_eq!(ids.canonical(&a), Some("mcp/one/read.file"));
    assert!(ids.insert("one", "read.file", &empty).is_err());
    assert!(
        Identities::default()
            .insert("one", "read.file", &HashSet::from([a]))
            .is_err()
    );
    assert!(qualified("one/two", "read").is_err());
    assert!(qualified("one", "read/*").is_err());
    for i in 2..MAX_TOOLS {
        ids.insert("one", &format!("tool{i}"), &empty).unwrap();
    }
    assert!(ids.insert("one", "overflow", &empty).is_err());
}
