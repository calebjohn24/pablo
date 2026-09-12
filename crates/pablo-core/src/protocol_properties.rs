//! Reproducible bounded mutation corpus; no network, threads or external fuzz tools.
use crate::{a2a, deployment, output, skills};
use serde_json::Value;

pub(crate) fn mutations(seed: &[u8]) -> Vec<Vec<u8>> {
    assert!(!seed.is_empty() && seed.len() <= 65536);
    let mut cases = vec![seed.to_vec(), Vec::new()];
    let mut state = 0x5041_424c_4f4b_3031u64;
    for index in 0..256 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let offset = state as usize % seed.len();
        let mut value = seed.to_vec();
        match index % 4 {
            0 => value.truncate(offset),
            1 => value[offset] ^= (state >> 32) as u8 | 1,
            2 => {
                value.insert(offset, (state >> 24) as u8);
            }
            _ => {
                value.remove(offset);
            }
        }
        assert!(value.len() <= 65537);
        cases.push(value);
    }
    cases
}

#[test]
fn k01_bounded_config_skill_schema_and_a2a_mutations_are_total_and_deterministic() {
    let root = std::env::temp_dir().join(format!("pablo-k01-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let result = std::panic::catch_unwind(|| {
        let seed = b"schema_version=1\n[credentials.gateway]\nconsumer='provider.vercel'\nsources=[{kind='environment',name='UNREAD_KEY'}]\n";
        for (index, bytes) in mutations(seed).into_iter().enumerate() {
            std::fs::write(root.join("entry.toml"), bytes).unwrap();
            let resolve = || {
                let mut request = deployment::ResolveRequest::new(root.clone(), "entry.toml");
                request
                    .path_bindings
                    .insert("workspace".into(), root.clone());
                deployment::resolve(request)
                    .map(|value| value.fingerprint().to_owned())
                    .map_err(|error| error.code)
            };
            if index == 0 {
                assert!(resolve().is_ok(), "valid configuration seed");
            }
            assert_eq!(resolve(), resolve());
        }
        assert!(
            skills::parse_metadata(
                "name: test\ndescription: Keep exact task details.\n",
                "test"
            )
            .is_ok()
        );
        assert!(output::compile(&serde_json::json!({"type":"string"})).is_ok());
        for bytes in mutations(
            b"name: test\ndescription: Keep exact task details.\nmetadata:\n  owner: fixture\n",
        ) {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                let parse = || {
                    skills::parse_metadata(text, "test").map(|v| serde_json::to_value(v).unwrap())
                };
                assert_eq!(parse(), parse());
            }
        }
        for bytes in mutations(br#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}"#) {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                assert_eq!(output::compile(&value).is_ok(), output::compile(&value).is_ok());
            }
        }
        let host = a2a::CardAdmission::new("https://agent.example.test/rpc", None, false).unwrap();
        assert!(
            host.validate(include_bytes!("../../../tests/fixtures/a2a/card.json"))
                .is_ok()
        );
        for bytes in mutations(include_bytes!("../../../tests/fixtures/a2a/card.json")) {
            assert_eq!(host.validate(&bytes).is_ok(), host.validate(&bytes).is_ok());
        }
        let seed = br#"{"jsonrpc":"2.0","id":"rpc","result":{"message":{"messageId":"m","contextId":"context","role":"ROLE_AGENT","parts":[{"text":"exact"}]}}}"#;
        assert!(a2a::wire::decode(seed, "rpc", a2a::wire::Mode::Send, false).is_ok());
        for bytes in mutations(seed) {
            let decode = || a2a::wire::decode(&bytes, "rpc", a2a::wire::Mode::Send, false).is_ok();
            assert_eq!(decode(), decode());
        }
    });
    std::fs::remove_dir_all(root).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
