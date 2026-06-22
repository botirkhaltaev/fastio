# AGENTS.md

Guidelines for AI agents working on `fastio`.

## Project Overview

`fastio` is a Rust library that exposes raw file I/O backends. It should stay format-agnostic: no checkpoint, tensor, or serialization logic belongs here.

## Build & Test

```bash
cargo check --all-targets
cargo check --no-default-features --all-targets
cargo check --no-default-features --features tokio --all-targets
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

## Architecture Rules

- Keep the public API small: `BlockingIo`, `AsyncIo`, `MmapIo`, range/write vocabulary, and backend config types.
- Do not reintroduce a public availability API or backend identity trait. Platform and runtime failures should be returned by the I/O operation.
- Keep `tokio` independent from Rayon. Tokio batch writes use scoped threads inside `block_in_place`.
- Keep Rayon usage inside the `sync` feature only.
- Gate optional storage types and APIs with their features (`mmap`, `pool`, `tokio`, `io-uring`).

## Style

- Rust edition 2024, minimum `rustc` 1.92.
- Prefer small, direct implementations over premature helpers.
- Use `std::io::Error` and `std::io::Result`; do not add a custom error type unless there is a concrete need.
- Keep `unsafe` limited to platform I/O and memory-mapping boundaries, with a nearby safety comment.

## PR Guidance

- Validate feature combinations, especially `--no-default-features` and `--features tokio`.
- Avoid new dependencies unless they are feature-gated and justified.
- Update README files for human-facing behavior changes and AGENTS.md for workflow/architecture changes.
