# AGENTS.md

Guidelines for agents editing the `sync` backend.

## Rules

- Keep the public API shaped like `std::fs` only where behavior matches: backend-owned `File` and `OpenOptions`, no module functions.
- Do not add Rayon to the `sync` feature unless a measured file API need appears.
- Keep platform-sensitive implementation code split by OS (`linux.rs`, `macos.rs`, `windows.rs`). `mod.rs` should only select and re-export the current platform.

## Validation

```bash
cargo check --features sync --all-targets
cargo test --features sync
```

On non-Linux systems, Linux-only paths should remain behind `#[cfg(target_os = "linux")]`.
