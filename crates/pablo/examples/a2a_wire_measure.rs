//! CPU-only bounded wire decoder, with full typed results materialized.
use pablo_core::a2a::wire::{Mode, decode};
use serde_json::{Value, json};
fn main() {
    let path = std::env::args().nth(1).expect("a2a_wire_measure WIRE_JSON");
    let vectors: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut measurements = serde_json::Map::new();
    for (name, mode) in [
        ("message_result", Mode::Send),
        ("task_result", Mode::Send),
        ("stream_artifact", Mode::Stream),
        ("cancel_result", Mode::Cancel),
    ] {
        let bytes = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"rpc","result":vectors[name]}))
            .unwrap();
        let mut samples = Vec::new();
        for i in 0..35 {
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                std::hint::black_box(
                    decode(std::hint::black_box(&bytes), "rpc", mode, false).unwrap(),
                );
            }
            if i >= 5 {
                samples.push(start.elapsed().as_secs_f64() * 1000.);
            }
        }
        samples.sort_by(f64::total_cmp);
        measurements.insert(name.into(),json!({"response_bytes":bytes.len(),"microseconds_per_decode":{"min":samples[0],"p50":samples[14],"p95":samples[28],"max":samples[29]}}));
    }
    println!(
        "{}",
        json!({"method":{"samples":30,"warmup":5,"decodes_per_sample":1000,"scope":"CPU-only SDK fixture JSONRPC decode, typed output, validation and bounds; excludes HTTP/SSE framing and remote execution."},"measurements":measurements})
    );
}
