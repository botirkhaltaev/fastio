# AGENTS.md

Guidelines for agents editing `src`.

## Boundaries

- Keep domain vocabulary in `range.rs` and `write.rs`; avoid growing `lib.rs` with implementation details.
- Keep backend modules focused on file I/O only.
- Preserve explicit backend choice; do not add a root default file API.
- Do not add format-specific behavior or policy decisions here.

## Feature Gates

- `tokio` code requires `feature = "tokio"`.
- `mmap::File`, `MmapRegion`, and `OwnedBytes::Mmap` require `feature = "mmap"`.
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
