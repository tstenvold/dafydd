#![allow(missing_docs)]
//! Benchmarks for the TCP subnet scanner.
//!
//! Scenarios covered:
//!
//! 1. **Refused sweep** — all 254 hosts on 127.0.0.0/24 at a port that
//!    is never open. Connection-refused responses are near-instant on loopback,
//!    measuring framework dispatch and collection overhead across concurrency caps.
//!
//! 2. **Probe round-trip** — a single host (127.0.0.1/32) with a local echo
//!    server. Measures the connect → write → read → match latency.
//!
//! 3. **Large subnet refused sweep** (Linux only) — all 65 534 hosts on
//!    127.0.0.0/16, forcing ~33 batches through the lazy host iterator.
//!    Measures batching overhead at scale; Linux-only because only Linux
//!    routes the full 127.0.0.0/8 loopback block.
//!
//! 4. **Probe size scaling** — single host with probes of 16, 1 024, and
//!    8 192 bytes. With `Arc<[u8]>` the per-task cost is O(1) regardless of
//!    probe size; this benchmark verifies that property.
//!
//! 5. **Probe timeout expiry** — single host that accepts but never responds.
//!    Measures the I/O timeout machinery — the dominant scenario on real
//!    slow networks.
//!
//! Run with:
//!     cargo bench --bench `tcp_scan`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, SamplingMode};
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

/// Spawn a local TCP server that accepts connections but never writes back.
/// Used to benchmark the I/O timeout path.
fn start_silent_server(rt: &Runtime) -> u16 {
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind silent server");
        let port = listener.local_addr().expect("no local addr").port();
        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    // Hold connection open without responding so the client's
                    // io_timeout fires.
                    tokio::spawn(async move {
                        let _stream = stream;
                        tokio::time::sleep(Duration::from_secs(300)).await;
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
    let silent_port = start_silent_server(&rt);

    let mut group = c.benchmark_group("tcp_scan");
    // 10 samples × ~1 s each = 10 s measurement window per scenario.
    // Network benchmarks have inherent OS-scheduler variance; this budget gives
    // a reliable median without unbounded wall-clock time.
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));
    group.sampling_mode(SamplingMode::Flat);

    // ── Refused sweep (all 254 hosts, no probe) ───────────────────────────
    // Measures how quickly the scanner can dispatch and drain a full /24 when
    // every host immediately refuses the connection.
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
                            Duration::from_millis(50),
                            max_concurrent,
                        )
                        .await
                        .expect("scan failed");
                        std::hint::black_box(matches)
                    }
                });
            },
        );
    }

    // ── Probe round-trip (single host, echo server) ───────────────────────
    // Measures end-to-end latency: connect → write probe → read response →
    // build DeviceMatch.
    group.bench_function("probe_roundtrip", |b| {
        b.to_async(&rt).iter(|| {
            let subnets = vec!["127.0.0.1/32".to_string()];
            async move {
                let matches = scan_subnets(
                    &subnets,
                    echo_port,
                    Some(b"ping"),
                    Duration::from_millis(200),
                    Duration::from_millis(500),
                    1,
                )
                .await
                .expect("scan failed");
                std::hint::black_box(matches)
            }
        });
    });

    // ── Large subnet refused sweep (/16 ≈ 65 k hosts, Linux only) ─────────
    // Forces ~33 batches through the lazy host iterator. Only meaningful on
    // Linux where the entire 127.0.0.0/8 block is loopback; on macOS/Windows
    // most addresses would hit connect_timeout instead of an instant refusal.
    #[cfg(target_os = "linux")]
    group.bench_function("refused_large_subnet", |b| {
        b.to_async(&rt).iter(|| async {
            let matches = scan_subnets(
                &["127.0.0.0/16".to_string()],
                1,
                None,
                Duration::from_millis(10),
                Duration::from_millis(10),
                500,
            )
            .await
            .expect("scan failed");
            std::hint::black_box(matches)
        });
    });

    // ── Probe size scaling ─────────────────────────────────────────────────
    // With Arc<[u8]> probe sharing, the per-task cost is a pointer bump
    // regardless of payload size. This benchmark verifies the curve is flat.
    for &probe_size in &[16_usize, 1024, 8192] {
        let probe_bytes = vec![0u8; probe_size];
        group.bench_with_input(
            BenchmarkId::new("probe_size_scaling", probe_size),
            &probe_size,
            |b, _| {
                let probe = probe_bytes.clone();
                b.to_async(&rt).iter(|| {
                    let probe = probe.clone();
                    async move {
                        let matches = scan_subnets(
                            &["127.0.0.1/32".to_string()],
                            echo_port,
                            Some(&probe),
                            Duration::from_millis(200),
                            Duration::from_millis(500),
                            1,
                        )
                        .await
                        .expect("scan failed");
                        std::hint::black_box(matches)
                    }
                });
            },
        );
    }

    // ── Probe timeout expiry (single host, no response) ──────────────────
    // Measures the I/O timeout machinery — the dominant scenario on real
    // networks where devices accept connections but respond slowly or not at all.
    group.bench_function("probe_timeout_expiry", |b| {
        b.to_async(&rt).iter(|| async {
            let matches = scan_subnets(
                &["127.0.0.1/32".to_string()],
                silent_port,
                Some(b"ping"),
                Duration::from_millis(200),
                Duration::from_millis(100),
                1,
            )
            .await
            .expect("scan failed");
            std::hint::black_box(matches)
        });
    });

    group.finish();
}

criterion_group!(benches, bench_tcp_scan);
criterion_main!(benches);
