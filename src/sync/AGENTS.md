# AGENTS.md

Guidelines for agents editing the `sync` backend.

## Rules

- Keep the public API shaped like `std::fs`: backend-owned `File`, `OpenOptions`, and module functions.
- Do not add Rayon to the `sync` feature unless a measured file API need appears.
- Linux O_DIRECT behavior should remain operation-time behavior, not construction-time availability checking.
- Do not split platform code or add abstractions unless duplication becomes a proven maintenance problem.

## Validation

```bash
cargo check --features sync --all-targets
cargo test --features sync
```

On non-Linux systems, Linux-only paths should remain behind `#[cfg(target_os = "linux")]`.
