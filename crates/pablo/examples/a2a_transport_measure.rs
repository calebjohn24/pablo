//! Compare pooled direct HTTP with the bounded task transport against one SDK peer.
use pablo_core::{
    CancellationToken,
    a2a::{
        CardAdmission,
        transport::{Request, TaskClient},
        wire,
    },
};
use serde_json::json;
use std::time::Duration;
use tokio::time::Instant;
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .expect("a2a_transport_measure LOOPBACK_RPC");
    let card = CardAdmission::new("https://agent.example.test/rpc", None, false)
        .unwrap()
        .validate(include_bytes!("../../../tests/fixtures/a2a/card.json"))
        .unwrap();
    let transport = TaskClient::fixture(&card, &url).unwrap();
    let direct = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .build()
        .unwrap();
    let mut measurements = serde_json::Map::new();
    for stream in [false, true] {
        let input = if stream { "assembly" } else { "measurement" };
        let parts = [wire::Part::text(input)];
        let modes = ["text/plain", "application/octet-stream"];
        let body = wire::send_parts_request(
            "rpc",
            "message",
            &parts,
            &wire::SendOptions {
                stream,
                accepted_output_modes: &modes,
                ..Default::default()
            },
        )
        .unwrap();
        let mut raw_samples = Vec::new();
        let mut bounded_samples = Vec::new();
        for sample in 0..35 {
            // Alternate paired order to expose ordering bias in a local peer.
            for bounded in if sample % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let start = Instant::now();
                if bounded {
                    let execution = transport
                        .execute(
                            Request {
                                parts: &parts,
                                accepted_output_modes: &modes,
                                stream,
                                trace: None,
                                deadline: Instant::now() + Duration::from_secs(3),
                                cleanup_deadline: Instant::now() + Duration::from_secs(4),
                            },
                            &CancellationToken::new(),
                            &mut (),
                        )
                        .await;
                    let result = execution.result.unwrap();
                    assert!(result.disposition.is_some());
                    std::hint::black_box(result);
                } else {
                    let response = direct
                        .post(&url)
                        .headers(body.headers.clone())
                        .body(body.body.clone())
                        .send()
                        .await
                        .unwrap();
                    assert!(response.status().is_success());
                    let bytes = response.bytes().await.unwrap();
                    assert!(!bytes.is_empty());
                    std::hint::black_box(bytes);
                }
                let ms = start.elapsed().as_secs_f64() * 1000.;
                if sample >= 5 {
                    if bounded {
                        bounded_samples.push(ms);
                    } else {
                        raw_samples.push(ms);
                    }
                }
            }
        }
        let stats = |mut samples: Vec<f64>| {
            samples.sort_by(f64::total_cmp);
            json!({"n":samples.len(),"min":samples[0],"p50":samples[14],"p95":samples[28],"max":samples[29]})
        };
        measurements.insert(if stream{"stream_artifact"}else{"immediate_message"}.into(),json!({"pooled_direct_http_ms":stats(raw_samples),"bounded_task_ms":stats(bounded_samples)}));
    }
    println!(
        "{}",
        json!({"method":{"samples":30,"warmup":5,"pairing":"alternating direct/bounded order per pair","reference":"pinned independent Python A2A SDK 1.0.2 loopback JSONRPC/SSE peer","baseline":"pooled reqwest HTTP through full response bytes; no typed decode/assembly","current":"pooled scoped TaskClient, bounded framing/typed lifecycle, no-op sink; construction excluded","scope":"local HTTP and SDK dispatcher only; no TLS/auth/model/supervisor/Collector or remote-network performance claim"},"measurements":measurements})
    );
}
