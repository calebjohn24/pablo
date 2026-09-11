//! CPU-only SSE framing and complete remote result assembly, excluding transport.
use pablo_core::a2a::{lifecycle::Lifecycle, sse::Decoder, wire::Mode};
use serde_json::{Value, json};
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("a2a_lifecycle_measure ASSEMBLY_JSON");
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut measurements = serde_json::Map::new();
    for large in [false, true] {
        let mut events = fixture["events"].as_array().unwrap().clone();
        if large {
            events[0]["artifact"]["parts"][0]["text"] = "x".repeat(32000).into();
        }
        let mut frames = events
            .into_iter()
            .map(|event| json!({"artifactUpdate":event}))
            .collect::<Vec<_>>();
        frames.push(json!({"statusUpdate":{"taskId":"remote-task","contextId":"remote-context","status":{"state":"TASK_STATE_COMPLETED"}}}));
        let raw = frames
            .iter()
            .map(|result| {
                format!(
                    "data: {}\n\n",
                    json!({"jsonrpc":"2.0","id":"rpc","result":result})
                )
            })
            .collect::<String>();
        let mut samples = Vec::new();
        for i in 0..35 {
            let start = std::time::Instant::now();
            for _ in 0..100 {
                let mut parser = Decoder::default();
                let mut lifecycle = Lifecycle::default();
                for byte in std::hint::black_box(raw.as_bytes()) {
                    if let Some(data) = parser.push(*byte).unwrap() {
                        lifecycle.ingest(&data, "rpc", Mode::Stream, false).unwrap();
                    }
                }
                parser.finish().unwrap();
                std::hint::black_box(lifecycle.finish().unwrap());
            }
            if i >= 5 {
                samples.push(start.elapsed().as_secs_f64() * 10000.);
            }
        }
        samples.sort_by(f64::total_cmp);
        measurements.insert(if large {"large"} else {"small"}.into(),json!({"raw_sse_bytes":raw.len(),"frames":frames.len(),"microseconds_per_lifecycle":{"min":samples[0],"p50":samples[14],"p95":samples[28],"max":samples[29]}}));
    }
    println!(
        "{}",
        json!({"method":{"samples":30,"warmup":5,"lifecycles_per_sample":100,"scope":"CPU-only incremental raw SSE framing, JSONRPC validation and bounded full artifact/result materialization; no HTTP, supervisor, provider or remote execution."},"measurements":measurements})
    );
}
