# tokio Backend

The `tokio` module provides a `tokio::fs`-like `File` and `OpenOptions` with positioned I/O methods.

File operations are async. Positioned writes use blocking positioned I/O inside Tokio blocking sections so callers can write independent file ranges in parallel.

This feature intentionally does not depend on Rayon.
