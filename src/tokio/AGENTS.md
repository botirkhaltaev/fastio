# AGENTS.md

Guidelines for agents editing the `tokio` backend.

## Rules

- Do not add Rayon to the `tokio` feature.
- Keep parallel positioned writes implemented with scoped threads inside `tokio::task::block_in_place` unless there is a measured reason to change it.
- Keep async public methods on `tokio::File` and `tokio::OpenOptions`; use blocking sections only around APIs that must use positioned synchronous writes.
- Do not reintroduce an `AsyncIo` trait or backend service object.

## Validation

```bash
cargo check --no-default-features --features tokio --all-targets
cargo test --no-default-features --features tokio
```
