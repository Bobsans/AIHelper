# Performance Guide

This document describes how to benchmark AIHelper commands and where the main hot paths are.

## Quick Benchmark

Run from project root:

```powershell
pwsh -File scripts/benchmark.ps1 -Iterations 5
```

Outputs:
- table in terminal with per-command min/avg/max latency
- markdown report at `benchmarks/latest.md`

## Benchmark Scope

Current suite covers:
- `file read` range extraction
- `search text` over Rust source tree
- `ctx symbols` with `review` preset
- `ctx pack` with `summary` preset
- `git changed`

## Recent Hardening

- `ctx pack` now uses streaming `WalkDir` traversal with early stop at limit.
- `ctx symbols` now scans files lazily and stops once file cap is reached.
- symbol extraction regexes are cached with `OnceLock` to avoid recompilation overhead.

## How To Compare Runs

1. Run benchmark before changes.
2. Run benchmark after changes with the same `Iterations` value.
3. Compare `benchmarks/latest.md` snapshots.
4. If any command regresses >20% average latency, inspect traversal and parsing logic first.
