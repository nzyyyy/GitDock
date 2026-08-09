# Performance checks

Performance checks are ignored by the normal Rust test run because they create 100,000 Git objects or files. Run them explicitly:

```bash
cargo test --manifest-path src-tauri/Cargo.toml benchmarks_ -- --ignored --nocapture
```

## 2026-08-09 baseline

- Hardware: 10-core Apple M1 Pro, 32 GB RAM
- System: macOS 15.7.7
- Git: Apple Git 2.50.1
- Rust: 1.96.0
- History fixture: 100,009 commits created with `git fast-import`, including an eight-parent octopus merge
- History first-page p95 (10 runs, 100 commits): 363 ms
- History page at offset 50,000 p95 (10 runs, 100 commits): 369 ms
- Ignored-tree fixture: 100,000 files under one ignored dependency directory
- Working-tree status p95 (10 runs): 27 ms

Both Git history measurements are below the 1-second page target. The ignored dependency tree remains excluded from ordinary status results.

Frontend history uses fixed-height windowing in both panes. Regression tests assert that loading 600 commits renders fewer than 60 rows per pane and that a long cross-bucket graph edge remains visible while scrolling.
