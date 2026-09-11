//! CPU-only A2A card admission baseline; no network or credential access.
use pablo_core::a2a::CardAdmission;
use serde_json::{Value, json};
fn main() {
    let path = std::env::args().nth(1).expect("a2a_card_measure CARD_JSON");
    let small = std::fs::read(path).unwrap();
    let mut large: Value = serde_json::from_slice(&small).unwrap();
    large["skills"]=json!((0..50).map(|n|json!({"id":format!("skill-{n}"),"name":"Fixture","description":"x".repeat(1000),"tags":["fixture"]})).collect::<Vec<_>>());
    let large = serde_json::to_vec(&large).unwrap();
    let host = CardAdmission::new("https://agent.example.test/rpc", None, false).unwrap();
    let mut report = serde_json::Map::new();
    for (name, bytes) in [("sdk_card", small), ("fifty_skills", large)] {
        let mut batches = Vec::new();
        for i in 0..35 {
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                let card =
                    std::hint::black_box(host.validate(std::hint::black_box(&bytes)).unwrap());
                assert_eq!(card.protocol_version, "1.0");
            }
            let micros = start.elapsed().as_secs_f64() * 1000.;
            if i >= 5 {
                batches.push(micros);
            }
        }
        batches.sort_by(f64::total_cmp);
        report.insert(name.into(),json!({"card_bytes":bytes.len(),"microseconds_per_admission":{"min":batches[0],"p50":batches[14],"p95":batches[28],"max":batches[29]}}));
    }
    println!(
        "{}",
        json!({"method":{"samples":30,"warmup":5,"admissions_per_sample":1000,"scope":"CPU-only card decode, host interface/security/extension validation and raw-byte SHA-256; excludes HTTP, credentials and task dispatch."},"measurements":report})
    );
}
