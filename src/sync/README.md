# sync Backend

The `sync` module provides `SyncIo`, a blocking positioned I/O backend for Linux, macOS, and Windows.

- Linux supports buffered reads and optional O_DIRECT reads.
- macOS and Windows use platform positioned-I/O APIs.
- Batch operations use Rayon and are only available when the `sync` feature is enabled.

Use `SyncOptions` to configure batch thread count, direct I/O behavior, and read-buffer allocation.
