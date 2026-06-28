#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    // Interpret the input as a sequence of (offset, length) pairs.
    let count = data[0] as usize;
    let mut slices = Vec::with_capacity(count.min(64));
    let mut i = 1;

    for _ in 0..count.min(64) {
        if i + 10 > data.len() {
            break;
        }
        let offset = u64::from_le_bytes([
            data[i],
            data[i + 1],
            data[i + 2],
            data[i + 3],
            data[i + 4],
            data[i + 5],
            data[i + 6],
            data[i + 7],
        ]);
        let len = u16::from_le_bytes([data[i + 8], data[i + 9]]) as usize;
        i += 10;

        let end = (i + len).min(data.len());
        let payload = &data[i..end];
        i = end;

        slices.push(fastio::WriteSlice::new(offset, payload));
    }

    // The function must not panic; any error is acceptable.
    let _ = fastio::WriteSlices::new(&slices);
});
