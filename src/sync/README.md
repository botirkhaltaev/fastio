# sync Backend

The `sync` module provides platform-owned `File` and `OpenOptions` implementations for Linux, macOS, and Windows.

- Linux, macOS, and Windows use platform positioned-I/O APIs.
- `File` implements `std::io::Read`, `Write`, and `Seek`.
- `File` and `OpenOptions` are allocator-generic. Default reads use pooled buffers when `pool` is enabled; call `OpenOptions::allocator(System)` to force heap-backed reads.
- Full platform implementations live in `linux.rs`, `macos.rs`, and `windows.rs`; `mod.rs` only selects and re-exports the active platform.

Use `File::read_at`, `File::read_exact_at`, `File::write_all_at`, and `File::write_slices_at(WriteSlices::new(...)? )` for positioned I/O.
