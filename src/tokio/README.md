# tokio Backend

The `tokio` module provides `Tokio`, an async backend implementing `AsyncIo`.

Reads use Tokio filesystem APIs and bounded task concurrency. Positioned writes use blocking positioned I/O inside Tokio blocking sections so callers can write independent file ranges in parallel.

This feature intentionally does not depend on Rayon.
