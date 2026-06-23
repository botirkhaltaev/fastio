# AGENTS.md

Guidelines for AI agents working on `fastio`.

## Project Overview

`fastio` is a Rust library that exposes explicit file I/O backend modules. It should stay format-agnostic: no checkpoint, tensor, or serialization logic belongs here.

## Build & Test

```bash
cargo check --all-targets
cargo check --no-default-features --all-targets
cargo check --no-default-features --features tokio --all-targets
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

## Architecture Rules

- Keep the public API small and backend-qualified: `sync::File`, `tokio::File`, `mmap::File`, Linux `uring::File`, their `OpenOptions`, and shared byte/write vocabulary.
- Do not add a root default `File`, root `OpenOptions`, backend free functions, or compatibility shims; users must opt into a concrete backend file type.
- Do not reintroduce a public availability API or backend identity trait. Platform and runtime failures should be returned by the I/O operation.
- Keep `tokio` independent from Rayon. Tokio batch writes use scoped threads inside `block_in_place`.
- Do not add Rayon unless there is a measured backend-specific need.
- Gate optional storage types and APIs with their features (`mmap`, `pool`, `tokio`, `io-uring`).

## Style

- Rust edition 2024, minimum `rustc` 1.92.
- Prefer small, direct implementations over premature helpers; duplicate simple platform code instead of adding wrapper-only functions.
- Use `std::io::Error` and `std::io::Result`; do not add a custom error type unless there is a concrete need.
- Keep `unsafe` limited to platform I/O and memory-mapping boundaries, with a nearby safety comment.

## PR Guidance

- Validate feature combinations, especially `--no-default-features` and `--features tokio`.
- Avoid new dependencies unless they are feature-gated and justified.
- Update README files for human-facing behavior changes and AGENTS.md for workflow/architecture changes.
