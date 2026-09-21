use obc_display::ls021::row_hash;

fn main() {
    use std::hint::black_box;
    use std::time::Instant;

    /// The pre-fix hash: plain word-folded FNV-1a with no premix.
    fn old_row_hash(row: &[u8]) -> u32 {
        let mut h: u32 = 0x811c_9dc5;
        let (words, remainder) = row.as_chunks::<4>();
        for w in words {
            h ^= u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            h = h.wrapping_mul(0x0100_0193);
        }
        for &b in remainder {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
        h
    }

    const STRIDE: usize = 240; // device-64: one byte per pixel
    const ROWS: usize = 320;
    // A deterministic pseudo-random device-64 plane (bytes ≤ 0x3F, like real content).
    let mut plane = vec![0u8; STRIDE * ROWS];
    let mut s: u64 = 0x243F_6A88_85A3_08D3;
    for b in plane.iter_mut() {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *b = ((s >> 33) as u8) & 0x3F;
    }

    /// Median time of one full-plane pass (one hash per row), over 200 passes.
    fn median_pass_ns(plane: &[u8], hash: fn(&[u8]) -> u32) -> u128 {
        const REPS: usize = 200;
        let mut times = Vec::with_capacity(REPS);
        for _ in 0..REPS {
            let t0 = Instant::now();
            let mut acc = 0u32;
            for row in plane.as_chunks::<STRIDE>().0 {
                acc = acc.wrapping_add(hash(black_box(row)));
            }
            black_box(acc);
            times.push(t0.elapsed().as_nanos());
        }
        times.sort_unstable();
        times[REPS / 2]
    }

    let old_ns = median_pass_ns(&plane, old_row_hash);
    let new_ns = median_pass_ns(&plane, row_hash);
    std::eprintln!(
        "row-hash full {STRIDE}x{ROWS} plane pass (median of 200): pre-fix word-FNV {:.1} us, premixed {:.1} us, ratio {:.2}x",
        old_ns as f64 / 1000.0,
        new_ns as f64 / 1000.0,
        new_ns as f64 / old_ns as f64
    );
}
