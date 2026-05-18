#![allow(missing_docs)]
use criterion::{criterion_group, criterion_main, Criterion, SamplingMode};
use dafydd::serial::probe::{probe_port, probe_port_all_bauds, sweep_all_ports};
use std::time::Duration;
use tokio::runtime::Runtime;

fn bench_serial_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("serial_scan");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));
    group.sampling_mode(SamplingMode::Flat);

    let rt = Runtime::new().unwrap();

    // Benchmark 1: Fast-failure path for a non-existent port.
    // Measures open() failure overhead only — no I/O or sleep occurs.
    group.bench_function("probe_port_missing", |b| {
        b.to_async(&rt).iter(|| async {
            let res = probe_port(
                "/dev/tty_does_not_exist_123",
                9600,
                b"ping",
                Duration::from_millis(10),
                None,
                None,
                None,
                None,
                None,
                None, // response_filter
            )
            .await;
            std::hint::black_box(res)
        });
    });

    // Benchmark 2: Async spawn_blocking wrapper with fast-failing ports.
    // Measures the tokio::task::spawn_blocking overhead and JoinHandle cost.
    group.bench_function("probe_port_all_bauds_missing", |b| {
        b.to_async(&rt).iter(|| async {
            let res = probe_port_all_bauds(
                "/dev/tty_does_not_exist_123".to_string(),
                vec![9600, 115_200].into(),
                b"ping".to_vec().into(),
                Duration::from_millis(5),
                None,
                None,
                None,
                None,
                None,
                None,
                None, // response_filter
            )
            .await;
            std::hint::black_box(res)
        });
    });

    // Benchmark 3: Port enumeration and JoinSet setup without any I/O.
    // Measures available_ports() enumeration + macOS tty/cu dedup + the cost
    // of spawning and draining a JoinSet that immediately returns Ok(None)
    // for every port (empty baud list → no open() call).
    group.bench_function("sweep_all_ports_overhead", |b| {
        b.to_async(&rt).iter(|| async {
            let res = sweep_all_ports(
                b"",
                &[], // no baud rates → no actual I/O
                Duration::from_millis(1),
                false,
                None,
                None,
                None,
                None,
                None, // port_filter
                None,
                None,
                None,
                None, // response_filter
            )
            .await;
            std::hint::black_box(res)
        });
    });
}

criterion_group!(benches, bench_serial_scan);
criterion_main!(benches);
