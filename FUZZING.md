# Fuzzing

This project uses `cargo-fuzz` to exercise validation logic and unsafe I/O boundaries with randomized inputs.

## Setup

Install `cargo-fuzz`:

```bash
cargo install cargo-fuzz
```

## Available targets

- `write_slices`: validates `WriteSlices::new` overlap and offset-overflow checks with random slice batches.

## Running a target

From the repository root:

```bash
cargo fuzz run write_slices
```

To run for a specific duration or number of iterations, use `cargo fuzz` options:

```bash
# Run for 60 seconds
cargo fuzz run write_slices -- -max_total_time=60

# Run 1 million iterations
cargo fuzz run write_slices -- -runs=1000000
```

## Reproducing a crash

When `cargo-fuzz` finds a crash, it writes the input to `fuzz/corpus/<target>/crash-*`. Reproduce with:

```bash
cargo fuzz run write_slices fuzz/corpus/write_slices/crash-<id>
```

## CI

Fuzz targets are compile-checked in the workspace. Long-running fuzzing is not currently run in CI because it is non-deterministic and time-consuming. Run it locally before major changes to validation logic.
