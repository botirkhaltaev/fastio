# AGENTS.md

Guidelines for agents editing the `tokio` backend.

## Rules

- Do not add Rayon to the `tokio` feature.
- Keep positioned I/O off Tokio runtime worker threads. Use `spawn_blocking` with owned buffers for blocking filesystem work.
- Keep async public methods on `tokio::File` and `tokio::OpenOptions` when the operation can avoid blocking the runtime. If a method is inherently a cheap synchronous handle query, make it synchronous instead of `async`.
- Keep batch positioned writes bounded; do not spawn one OS thread per write slice for untrusted batch sizes.
- Do not reintroduce an `AsyncIo` trait or backend service object.
- Internal helpers must be as clean and purposeful as public methods. Do not hide simple positioned reads/writes behind helper wrappers unless the helper isolates real platform-specific behavior or repeated complexity.

## Validation

```bash
cargo check --no-default-features --features tokio --all-targets
cargo test --no-default-features --features tokio
```
