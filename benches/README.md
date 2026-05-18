# dafydd benchmarks

Run with:
```bash
cargo bench --bench tcp_scan
cargo bench --bench serial_scan
cargo bench --bench usb_scan
```

Results land in `target/criterion/`. Open `target/criterion/report/index.html`
for the rendered comparison views.

## `tcp_scan`

Four scenarios, all using loopback listeners so the benches are reproducible
without network setup:

### `probe_and_identify`

Three echo-style listeners. One responds with `b"sn:ES123DFD3\r\n"` (the
target); two reply with non-matching strings (decoys). The bench probes each
with `C0AMSF` and a `response_filter` of `b"ES123DFD3"`. Measures correct-
identification latency for the "device is somewhere in this group" use case.

### `tarpit_timeout`

Listener accepts the connection but never writes. The probe must hit
`io_timeout` cleanly. Measures the timeout fast-path — the dominant scenario
on real LANs where misconfigured devices leave connections half-open.

### `dafydd_sweep_sparse_24`

Full `scan_subnets("127.0.0.0/24", port=<target>)` sweep with a probe + filter.
253 hosts refuse instantly (loopback), one matches. Measures dafydd's actual
priority probe + concurrency pipeline — what users get out of the box.

### `baseline_naive_sweep_24`

Sequential connect/probe loop over the same 254 addresses, no concurrency,
no priority heuristics. Equivalent to a pure-asyncio user implementation.
This is the speedup ceiling for dafydd: `dafydd_sweep_sparse_24` should be
*at least* the order of `max_concurrent` faster, modulo OS limits.

## What the previous benches measured (and why they were dropped)

- **`refused_254_hosts`** measured loopback connection-refused throughput.
  Loopback refusals are ~100× faster than real LAN refusals; the number was
  unrepresentative.
- **`refused_large_subnet`** (Linux only, 127.0.0.0/16) measured /16 batch
  dispatch. Real users don't sweep /16 subnets — most LANs are /24 and a /16
  is now a hard limit by config (`subnet_prefix`). The bench tested code that
  is now gated.
- **`probe_size_scaling`** measured `Arc<[u8]>` sharing efficiency. That's an
  implementation detail with no behavioural impact for callers; if it ever
  regresses, the slowdown shows up in `probe_and_identify` anyway.
