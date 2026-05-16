#![allow(missing_docs)]
use criterion::{criterion_group, criterion_main, Criterion};
use dafydd::serial::probe::{probe_port, probe_port_all_bauds};
use std::time::Duration;
use tokio::runtime::Runtime;

fn bench_serial_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("serial_scan");
    group.sample_size(10);

    // Benchmark 1: Dispatch capability of a non-existent port (measure framework overhead)
    // Fast failure logic.
    group.bench_function("probe_port_missing", |b| {
        b.iter(|| {
            let res = probe_port(
                "/dev/tty_does_not_exist_123",
                9600,
                b"ping",
                Duration::from_millis(10),
            );
            std::hint::black_box(res)
        });
    });

    let rt = Runtime::new().unwrap();
    group.bench_function("probe_port_all_bauds_missing", |b| {
        b.to_async(&rt).iter(|| async {
            let res = probe_port_all_bauds(
                "/dev/tty_does_not_exist_123".to_string(),
                vec![9600, 115_200],
                b"ping".to_vec(),
                Duration::from_millis(5),
            )
            .await;
            std::hint::black_box(res)
        });
    });
}

criterion_group!(benches, bench_serial_scan);
criterion_main!(benches);
