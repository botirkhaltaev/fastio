# sync Backend

The `sync` module provides a `std::fs`-like `File`, `OpenOptions`, and module functions for Linux, macOS, and Windows.

- Linux supports optional O_DIRECT with `OpenOptions::direct_io(true)`.
- macOS and Windows use platform positioned-I/O APIs.
- `File` implements `std::io::Read`, `Write`, and `Seek`.
- Platform-specific open and positioned-I/O code lives in `linux.rs`, `macos.rs`, and `windows.rs`; `mod.rs` owns the shared public API.

Use `File::read_at`, `File::read_exact_at`, `File::write_all_at`, and `File::write_slices_at` for positioned I/O.
