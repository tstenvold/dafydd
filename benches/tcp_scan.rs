#![allow(missing_docs)]
//! Benchmarks for the TCP subnet scanner.
//!
//! These scenarios target the *real* use case: "I have a device that should
//! be at X. If it isn't, scan the rest of the subnet and find it by probe
//! response." Each bench measures something the caller will care about:
//! sparse-LAN discovery latency, tarpit timeout handling, correct-match
//! latency, and a baseline against a naive sweep.
//!
//! Run with:
//!     cargo bench --bench `tcp_scan`
//!
//! See `benches/README.md` for what each bench represents.
//!
//! Loopback alias addresses (127.0.0.2, 127.0.0.3, …) are used on Linux —
//! the entire 127/8 block routes back. On macOS and Windows only 127.0.0.1
//! is loopback, so the "sparse subnet" scenarios bind multiple listeners
//! to 127.0.0.1 on different ports instead.

use criterion::{criterion_group, criterion_main, Criterion, SamplingMode};
use dafydd::tcp::scan::{probe_addr, scan_subnets};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;

/// Listener that replies with `response` on every connection (after reading
/// the client's probe). Returns the bound `SocketAddr`.
fn spawn_echo_listener(rt: &Runtime, response: &'static [u8]) -> SocketAddr {
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind echo listener");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            loop {
                if let Ok((mut s, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let mut buf = [0u8; 1024];
                        let _ = s.read(&mut buf).await;
                        let _ = s.write_all(response).await;
                    });
                }
            }
        });
        addr
    })
}

/// Listener that accepts but never replies — exercises the `io_timeout` path.
fn spawn_tarpit_listener(rt: &Runtime) -> SocketAddr {
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind tarpit listener");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let _stream = stream;
                        tokio::time::sleep(Duration::from_mins(5)).await;
                    });
                }
            }
        });
        addr
    })
}

/// Naive baseline: connect to each `addr`, send probe, read until close,
/// match on substring. Equivalent to what users write in pure asyncio.
async fn naive_sweep(
    addrs: &[SocketAddr],
    probe: &[u8],
    match_substring: &[u8],
    connect_timeout: Duration,
    io_timeout: Duration,
) -> Vec<SocketAddr> {
    let mut found = Vec::new();
    for &addr in addrs {
        let Ok(Ok(mut s)) =
            tokio::time::timeout(connect_timeout, tokio::net::TcpStream::connect(addr)).await
        else {
            continue;
        };
        let _ = s.write_all(probe).await;
        let result = tokio::time::timeout(io_timeout, async {
            let mut buf = Vec::with_capacity(4096);
            let mut tmp = [0u8; 4096];
            loop {
                match s.read(&mut tmp).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            buf
        })
        .await;
        if let Ok(buf) = result {
            if buf
                .windows(match_substring.len())
                .any(|w| w == match_substring)
            {
                found.push(addr);
            }
        }
    }
    found
}

#[allow(clippy::too_many_lines)]
fn bench_tcp_scan(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");

    // One listener that responds with a matching identifier.
    let target_addr = spawn_echo_listener(&rt, b"sn:ES123DFD3\r\n");
    // Two listeners that respond with non-matching identifiers (decoys).
    let decoy1 = spawn_echo_listener(&rt, b"sn:OTHER001\r\n");
    let decoy2 = spawn_echo_listener(&rt, b"sn:OTHER002\r\n");
    // Tarpit: accepts but never responds (exercises io_timeout).
    let tarpit = spawn_tarpit_listener(&rt);

    let mut group = c.benchmark_group("tcp_scan");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));
    group.sampling_mode(SamplingMode::Flat);

    // ── Scenario 1: probe-and-identify (sparse, decoys + target) ──────────
    // Three listeners respond; one matches the response_filter. Measures
    // correct-identification latency under realistic probe semantics.
    group.bench_function("probe_and_identify", |b| {
        b.to_async(&rt).iter(|| {
            let addrs: Vec<SocketAddr> = vec![decoy1, decoy2, target_addr];
            async move {
                let mut matches = Vec::new();
                let filter: Arc<[u8]> = Arc::from(&b"ES123DFD3"[..]);
                for addr in addrs {
                    if let Ok(Some(m)) = probe_addr(
                        addr,
                        Some(b"C0AMSF\r\n"),
                        Duration::from_millis(200),
                        Duration::from_millis(500),
                        None,
                        Some(&filter),
                        None,
                    )
                    .await
                    {
                        matches.push(m);
                    }
                }
                std::hint::black_box(matches)
            }
        });
    });

    // ── Scenario 2: tarpit timeout handling ─────────────────────────────────
    // The listener accepts but never writes — probe must hit io_timeout
    // cleanly, not deadlock. Measures the timeout fast-path.
    group.bench_function("tarpit_timeout", |b| {
        b.to_async(&rt).iter(|| async {
            let result = probe_addr(
                tarpit,
                Some(b"ping"),
                Duration::from_millis(200),
                Duration::from_millis(100),
                None,
                None,
                None,
            )
            .await
            .expect("probe_addr failed");
            std::hint::black_box(result)
        });
    });

    // ── Scenario 3: dafydd full-subnet sweep (sparse /24) ──────────────────
    // 127.0.0.0/24, one live host at 127.0.0.1 on the target port; the
    // remaining 253 IPs refuse. Measures dafydd's priority probe + sweep
    // dispatch on the realistic "3-devices-in-a-subnet" pattern.
    let target_port = target_addr.port();
    group.bench_function("dafydd_sweep_sparse_24", |b| {
        b.to_async(&rt).iter(|| {
            let subnets = vec!["127.0.0.0/24".to_string()];
            async move {
                let matches = scan_subnets(
                    &subnets,
                    &[target_port],
                    Some(b"C0AMSF\r\n"),
                    Duration::from_millis(50),
                    Duration::from_millis(200),
                    500,
                    false, // skip ARP (loopback has no ARP)
                    None,
                    None,
                    Some(Arc::from(&b"ES123DFD3"[..])),
                    None,
                )
                .await
                .expect("scan failed");
                std::hint::black_box(matches)
            }
        });
    });

    // ── Scenario 4: naive asyncio-equivalent baseline ──────────────────────
    // Same target_port across all 254 hosts of 127.0.0.0/24, but probed
    // sequentially the way a user would in plain asyncio. Establishes the
    // speedup ceiling: dafydd's priority + concurrency should win here.
    group.bench_function("baseline_naive_sweep_24", |b| {
        b.to_async(&rt).iter(|| {
            let mut addrs: Vec<SocketAddr> = (1u8..=254)
                .map(|n| SocketAddr::from(([127, 0, 0, n], target_port)))
                .collect();
            // Replace the entry that corresponds to our actual listener.
            for a in &mut addrs {
                if *a == SocketAddr::from(([127, 0, 0, 1], target_port)) {
                    *a = target_addr;
                }
            }
            async move {
                let found = naive_sweep(
                    &addrs,
                    b"C0AMSF\r\n",
                    b"ES123DFD3",
                    Duration::from_millis(50),
                    Duration::from_millis(200),
                )
                .await;
                std::hint::black_box(found)
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_tcp_scan);
criterion_main!(benches);
