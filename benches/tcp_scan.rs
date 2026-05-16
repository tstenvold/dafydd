#![allow(missing_docs)]
//! Benchmarks for the TCP subnet scanner.
//!
//! Two scenarios are covered:
//!
//! 1. **Refused sweep** — all 254 hosts on 127.0.0.0/24 at a port that
//!    is never open. Connection-refused responses are near-instant on loopback,
//!    so this measures framework dispatch and collection overhead across a range
//!    of concurrency caps.
//!
//! 2. **Probe round-trip** — a single host (127.0.0.1/32) with a local echo
//!    server. Measures the write → read → match latency for one connection.
//!
//! Run with:
//!     cargo bench --bench `tcp_scan`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use dafydd::tcp::scan::scan_subnets;
use std::time::Duration;
use tokio::runtime::Runtime;

/// Spawn a local TCP server that accepts connections, reads whatever arrives,
/// and writes `response` back. Returns the port it is bound to.
fn start_echo_server(rt: &Runtime, response: &'static [u8]) -> u16 {
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind echo server");
        let port = listener.local_addr().expect("no local addr").port();
        tokio::spawn(async move {
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        use tokio::io::AsyncWriteExt;
                        let _ = stream.write_all(response).await;
                    });
                }
            }
        });
        port
    })
}

fn bench_tcp_scan(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let echo_port = start_echo_server(&rt, b"OK");

    let mut group = c.benchmark_group("tcp_scan");
    // Network benchmarks have inherent OS-scheduler variance; 10 samples still
    // gives a reliable median and avoids very long bench runs.
    group.sample_size(10);

    // ── Refused sweep (all 254 hosts, no probe) ───────────────────────────
    // Measures how quickly the scanner can dispatch and drain a full /24 when
    // every host immediately refuses the connection. The semaphore and JoinSet
    // overhead is the dominant cost here.
    for &concurrency in &[100_usize, 250, 500] {
        group.bench_with_input(
            BenchmarkId::new("refused_254_hosts", concurrency),
            &concurrency,
            |b, &max_concurrent| {
                b.to_async(&rt).iter(|| {
                    let subnets = vec!["127.0.0.0/24".to_string()];
                    async move {
                        let matches = scan_subnets(
                            &subnets,
                            // Port 1 is always refused on loopback (privileged,
                            // nothing ever listens here).
                            1,
                            None,
                            Duration::from_millis(50),
                            max_concurrent,
                        )
                        .await
                        .expect("scan failed");
                        criterion::black_box(matches)
                    }
                });
            },
        );
    }

    // ── Probe round-trip (single host, echo server) ───────────────────────
    // Measures end-to-end latency: connect → write probe → read response →
    // build DeviceMatch. Useful for tracking per-connection overhead.
    group.bench_function("probe_roundtrip", |b| {
        b.to_async(&rt).iter(|| {
            let subnets = vec!["127.0.0.1/32".to_string()];
            async move {
                let matches = scan_subnets(
                    &subnets,
                    echo_port,
                    Some(b"ping"),
                    Duration::from_millis(500),
                    1,
                )
                .await
                .expect("scan failed");
                criterion::black_box(matches)
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_tcp_scan);
criterion_main!(benches);
