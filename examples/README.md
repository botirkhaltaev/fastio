# fastio examples

Each example is feature-gated and can be run with `cargo run --example <name> --features <features>`.

| Example                 | Features required | Description                                          |
| :---------------------- | :---------------- | :--------------------------------------------------- |
| `sync_read_write`       | `sync`            | Synchronous read, write, positioned I/O, and batches |
| `tokio_read_write`      | `tokio`           | Async read and write via Tokio                       |
| `mmap_map`              | `mmap`            | Full-file and ranged memory maps                     |
| `uring_batch_read`      | `io-uring`        | Linux-only io_uring batch read                       |
| `write_slices`          | `sync`            | Non-overlapping batch writes (any write backend)      |

Run a specific example:

```bash
cargo run --example sync_read_write --features sync
```

The `io-uring` example is additionally gated to Linux; it compiles but prints a
message on other platforms.
