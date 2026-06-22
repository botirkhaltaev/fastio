# AGENTS.md

Guidelines for agents editing `src`.

## Boundaries

- Keep domain vocabulary in `range.rs`, `write.rs`, and `traits.rs`; avoid growing `lib.rs` with implementation details.
- Keep backend modules focused on file I/O only.
- Do not add format-specific behavior or policy decisions here.

## Feature Gates

- `AsyncIo` and `tokio` code require `feature = "tokio"`.
- `MmapIo`, `Mmap`, `MmapRegion`, and `OwnedBytes::Mmap` require `feature = "mmap"`.
- Pool-backed buffers require `feature = "pool"`.
- `io_uring` is Linux-only and requires `feature = "io-uring"`.

## Validation

Run at least:

```bash
cargo check --all-targets
cargo check --no-default-features --all-targets
cargo check --no-default-features --features tokio --all-targets
```

Run tests and clippy before finalizing broader changes.
