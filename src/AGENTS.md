# AGENTS.md

Guidelines for agents editing `src`.

## Boundaries

- Keep shared vocabulary minimal; avoid growing `lib.rs` with implementation details.
- Keep backend modules focused on file I/O only.
- Preserve explicit backend choice; do not add a root default file API.
- Do not add production free functions or backend-level free functions; operations belong on concrete backend file types or purposeful associated methods.
- Treat internal functions and types like API surface. They should be composable, clean, general, and purposeful, not premature wrappers around one or two call sites.
- Prefer inlining simple logic. Add an internal helper only when it encodes a real invariant, isolates platform/unsafe complexity, or represents a meaningful reusable operation.
- Do not add format-specific behavior or policy decisions here.

## Feature Gates

- `tokio` code requires `feature = "tokio"`.
- `mmap::File`, `MmapRegion`, and `OwnedBytes::Mmap` require `feature = "mmap"`.
- Pool-backed buffers require `feature = "pool"`.
- `io_uring` is Linux-only and requires `feature = "io-uring"`.

## Allocation

- Read-capable backends must use their configured `Allocator` for owned read buffers.
- `DefaultAllocator` is `Pool` when `pool` is enabled and `System` otherwise.
- Keep mmap separate from allocator-backed reads; mapped bytes are not pooled buffers.

## Validation

Run at least:

```bash
cargo check --all-targets
cargo check --no-default-features --all-targets
cargo check --no-default-features --features tokio --all-targets
```

Run tests and clippy before finalizing broader changes.
