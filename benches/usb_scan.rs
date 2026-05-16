#![allow(missing_docs)]
use criterion::{criterion_group, criterion_main, Criterion, SamplingMode};
use dafydd::runtime::runtime;
use std::collections::HashMap;
use std::time::Duration;

fn bench_usb_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("usb_scan");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));
    group.sampling_mode(SamplingMode::Flat);

    // Benchmark 1: Raw OS USB enumeration cost.
    // Baseline for the framework overhead below.
    group.bench_function("nusb_list_devices_overhead", |b| {
        b.iter(|| {
            let res = runtime().block_on(async { nusb::list_devices().await });
            std::hint::black_box(res)
        });
    });

    // Benchmark 2: Full enumeration path including HashMap and String allocation.
    // Isolates the metadata-construction overhead (vendor_id formatting, optional
    // string copies) that the raw nusb benchmark omits.
    group.bench_function("enumerate_with_metadata", |b| {
        b.iter(|| {
            let res = runtime().block_on(async {
                let Ok(devices) = nusb::list_devices().await else {
                    return vec![];
                };
                let mut out: Vec<HashMap<String, String>> = Vec::new();
                for device in devices {
                    let mut info: HashMap<String, String> = HashMap::with_capacity(6);
                    info.insert(
                        "vendor_id".to_owned(),
                        format!("{:#06x}", device.vendor_id()),
                    );
                    info.insert(
                        "product_id".to_owned(),
                        format!("{:#06x}", device.product_id()),
                    );
                    if let Some(m) = device.manufacturer_string() {
                        info.insert("manufacturer".to_owned(), m.to_owned());
                    }
                    if let Some(p) = device.product_string() {
                        info.insert("product".to_owned(), p.to_owned());
                    }
                    if let Some(s) = device.serial_number() {
                        info.insert("serial_number".to_owned(), s.to_owned());
                    }
                    out.push(info);
                }
                out
            });
            std::hint::black_box(res)
        });
    });

    group.finish();
}

criterion_group!(benches, bench_usb_scan);
criterion_main!(benches);
