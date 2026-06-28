# tokio Backend

The `tokio` module provides async `File` and `OpenOptions` with positioned I/O methods.

Regular filesystem operations such as open, clone, metadata, set length, sync, and permissions delegate to `tokio::fs`. Positioned reads and writes move a `try_clone`d `std::fs::File` into `tokio::task::spawn_blocking` so they do not block runtime worker threads. On Unix, positioned reads and writes use `pread`/`pwrite` which are cursor-independent; on Windows, platform helpers save/restore the file position. Batch writes use bounded worker waves inside one blocking task.

Default reads allocate from the internal buffer pool.

This feature intentionally does not depend on Rayon.
