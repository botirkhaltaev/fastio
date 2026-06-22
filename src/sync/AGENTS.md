# AGENTS.md

Guidelines for agents editing the `sync` backend.

## Rules

- Keep Rayon usage contained to this module and the `sync` feature.
- Preserve platform-specific files: `linux.rs`, `macos.rs`, and `windows.rs`.
- Linux O_DIRECT behavior should remain operation-time behavior, not construction-time availability checking.
- Do not share platform code by adding abstractions unless duplication becomes a proven maintenance problem.

## Validation

```bash
cargo check --features sync --all-targets
cargo test --features sync
```

On non-Linux systems, Linux-only paths should remain behind `#[cfg(target_os = "linux")]`.
