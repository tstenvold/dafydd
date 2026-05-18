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

    // Collect device data once outside the timed loop so that the benchmark
    // measures only the HashMap and String allocation work, not the OS call.
    // This makes results comparable across runs regardless of device count.
    type DeviceData = (u16, u16, u8, Option<String>, Option<String>, Option<String>);
    let devices: Vec<DeviceData> = runtime().block_on(async {
        let Ok(devs) = nusb::list_devices().await else {
            return vec![];
        };
        devs.map(|d| {
            (
                d.vendor_id(),
                d.product_id(),
                d.class(),
                d.manufacturer_string().map(str::to_owned),
                d.product_string().map(str::to_owned),
                d.serial_number().map(str::to_owned),
            )
        })
        .collect()
    });

    // Benchmark 2: Metadata-construction overhead per device.
    // Isolates the HashMap allocation and String formatting that the raw nusb
    // benchmark omits. Device data is pre-collected; only construction is timed.
    group.bench_function("enumerate_with_metadata", |b| {
        b.iter(|| {
            let mut out: Vec<HashMap<String, String>> = Vec::new();
            for (vid, pid, class, mfg, prod, sn) in &devices {
                let mut info: HashMap<String, String> = HashMap::with_capacity(7);
                info.insert("vendor_id".to_owned(), format!("{:#06x}", vid));
                info.insert("product_id".to_owned(), format!("{:#06x}", pid));
                info.insert("device_class".to_owned(), class.to_string());
                if let Some(m) = mfg {
                    info.insert("manufacturer".to_owned(), m.clone());
                }
                if let Some(p) = prod {
                    info.insert("product".to_owned(), p.clone());
                }
                if let Some(s) = sn {
                    info.insert("serial_number".to_owned(), s.clone());
                }
                out.push(info);
            }
            std::hint::black_box(out)
        });
    });

    group.finish();
}

criterion_group!(benches, bench_usb_scan);
criterion_main!(benches);
