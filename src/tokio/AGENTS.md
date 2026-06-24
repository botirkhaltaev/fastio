# AGENTS.md

Guidelines for agents editing the `tokio` backend.

## Rules

- Do not add Rayon to the `tokio` feature.
- Do not use `tokio::fs`. The backend owns a `std::fs::File` and runs all I/O via `tokio::task::spawn_blocking` with `try_clone`d handles.
- Keep all I/O off Tokio runtime worker threads via `spawn_blocking` with owned data.
- Public methods on `tokio::File` and `tokio::OpenOptions` that perform OS filesystem work should be async.
- Keep batch positioned writes bounded; do not spawn one OS thread per write slice for untrusted batch sizes.
- Do not reintroduce an `AsyncIo` trait or backend service object.
- Internal helpers must be as clean and purposeful as public methods. Do not hide simple positioned reads/writes behind helper wrappers unless the helper isolates real platform-specific behavior or repeated complexity.

## Validation

```bash
cargo check --no-default-features --features tokio --all-targets
cargo test --no-default-features --features tokio
```
