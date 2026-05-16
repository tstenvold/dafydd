#![allow(missing_docs)]
use criterion::{criterion_group, criterion_main, Criterion};

// Note: USB discovery is currently tightly bound inside the Python trait `UsbDiscovery.discover`.
// Since that returns a PyResult, we benchmark raw `nusb::list_devices` to track the raw baseline
// that the framework wraps.

fn bench_usb_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("usb_scan");
    group.sample_size(20);

    group.bench_function("nusb_list_devices_overhead", |b| {
        b.iter(|| {
            // Evaluates how fast the underlying OS USB bus can be iterated.
            let res = nusb::list_devices();
            std::hint::black_box(res)
        });
    });
}

criterion_group!(benches, bench_usb_scan);
criterion_main!(benches);
