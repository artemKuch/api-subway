# Performance benchmarks

Benchmarks use `scripts/benchmark.py` and a release build. The synthetic Next.js fixture makes every source file a route handler, so endpoint density is intentionally higher than in a typical application. Fixture creation time is reported separately and excluded from analysis timings.

## 2026-07-19 baseline

Environment:

- Darwin 27.0.0, arm64
- `rustc 1.97.0`
- release binary: 8,921,568 bytes
- binary SHA-256: `9dd3c46252c4bed9b6031aed05756ac46b1377e9a426b6c53daaa1df61add574`

| Source files | Approximate LOC | Warmups | Measured rounds | Min | Median | Max | Median throughput |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 100,000 | 1 | 5 | 0.0893 s | 0.0947 s | 0.0992 s | 10,555.4 files/s |
| 10,000 | 1,000,000 | 1 | 3 | 0.6273 s | 0.6394 s | 0.6508 s | 15,640.6 files/s |

These measurements satisfy the v0.1 orientation budgets of under one second for 1,000 files / 100k LOC and under five seconds for 10,000 files / 1M LOC on this machine. They are recorded evidence, not CI pass/fail thresholds.

Reproduce the two profiles:

```bash
cargo build --release --locked -p api-subway
python3 scripts/benchmark.py --binary target/release/api-subway
python3 scripts/benchmark.py --binary target/release/api-subway --files 10000 --lines-per-file 100 --rounds 3
```
