//! **LS021B7DD02 source-bus wire pack** — the host-tested RGB222 → panel-wire transform the
//! FLPR backend drains (issue #154, epic #149).
//!
//! The sibling of [`device64_to_rgb565`](crate::device64_to_rgb565): where that expands a
//! device-64 ([`FbDevice64`](crate::FbDevice64)) byte back to RGB565 for an ST7789, this packs a
//! whole **row** of device-64 bytes into the LS021's parallel **source bus** wire words — the
//! format the FLPR coprocessor clocks out over `BSP`/`BCK` + the 6 data lines (`R0/G0/B0`,
//! `R1/G1/B1`). It is the "host-tested Rust pack fn" the FLPR epic deliberately keeps off the C
//! blob: the trickiest bit (the area-gradation split + the odd/even column interleave + the
//! pre-shift to GPIO bit positions) lives here, unit-tested against the M33 `PanelBus` reference,
//! so the bare-metal FLPR side stays a dumb `store → pulse BCK` loop.
//!
//! ## What a packed row is
//!
//! The panel writes each pixel **row** as two *area planes* selected by the gate clock's level:
//! an **MSB** plane (the 2/3-area block) and an **LSB** plane (the 1/3-area block). Each plane is
//! shifted in as one **sub-line** of [`BCK_PER_SUBLINE`] words. So one row packs to
//! [`ROW_WORDS`] u32s: the **MSB sub-line** at `[0..BCK_PER_SUBLINE)` then the **LSB sub-line** at
//! `[BCK_PER_SUBLINE..2·BCK_PER_SUBLINE)`, which is exactly how the FLPR reads `buf[i].ptr` (MSB at
//! `[0..len)`, LSB at `[len..2·len)`, `len = BCK_PER_SUBLINE`).
//!
//! ## What one word is
//!
//! Each word is one **pixel pair**: the even-`x` pixel on the `*0` lines (`R0/G0/B0`) and the
//! odd-`x` pixel on the `*1` lines (`R1/G1/B1`). A packed word holds those 6 data bits already
//! shifted to their P2 GPIO positions — `bit0 R0, bit1 R1, bit2 G0, bit3 G1, bit4 B0, bit5 B1`
//! (`= DATA_MASK 0x3F`) — so the FLPR presents a column with one `OUTCLR (~w & 0x3F)` + one
//! `OUTSET (w & 0x3F)` and no bit-twiddling. `BCK` is *not* in the word; it is the FLPR's own pulse.
//! The 4 trailing dummy/flush columns of each sub-line are black (`0`).
//!
//! **The panel is DDR** (issue #155): it latches the source bus on *both* `BCK` edges, so the FLPR
//! drains these words **one per edge** — word `2k` before the rising edge, word `2k+1` before the
//! falling — clocking the 120 pairs out in ~60 `BCK` cycles. The pack itself is edge-agnostic (it
//! just lays the pairs out in order); the rising/falling split lives in the FLPR's `drive_subline`
//! and the M33 `PanelBus`. (The original single-edge drive held each pair across a whole `BCK`
//! period and the panel captured it twice → half horizontal resolution + 32 colours.)
//!
//! ## Area-gradation split
//!
//! Each channel's device-64 level is 2 bits (`0..=3`): the **MSB plane** carries the high bit
//! (`level >> 1`, the 2/3-area block), the **LSB plane** the low bit (`level & 1`, the 1/3-area
//! block). This mirrors `PanelBus::plane_bits` / `fill_with` (`src/ls021.rs` on the board, the
//! analyzer-verified golden reference from epic #139) — the test module re-derives that split
//! independently and asserts byte-for-byte agreement.

/// Panel width in pixels — 240 columns, clocked as 120 pixel pairs per sub-line.
pub const WIDTH: usize = 240;
/// Data columns clocked per sub-line: `WIDTH / 2` pixels-per-`BCK` = **120**.
pub const COLS_PER_SUBLINE: usize = WIDTH / 2;
/// `BCK` words per sub-line: 120 data + **4** trailing dummy/flush (the datasheet horizontal
/// chart clocks 124 `BCK`/line; the dummies push the last pixels through the source shift
/// register). Also the FLPR's per-sub-line `len`.
pub const BCK_PER_SUBLINE: usize = COLS_PER_SUBLINE + 4;
/// Words in one full **row** buffer: the MSB sub-line followed by the LSB sub-line.
pub const ROW_WORDS: usize = 2 * BCK_PER_SUBLINE;

/// Pack one pixel pair (two device-64 bytes, `0b00_RR_GG_BB`) into one source-bus word for the
/// given area plane. `even` is the even-`x` pixel → `R0/G0/B0` (bits 0/2/4); `odd` is the
/// odd-`x` pixel → `R1/G1/B1` (bits 1/3/5). `msb` selects the area-gradation bit
/// (`level >> 1` for the 2/3-area MSB plane, `level & 1` for the 1/3-area LSB plane).
#[inline]
fn pack_pair(even: u8, odd: u8, msb: bool) -> u32 {
    let shift = if msb { 1 } else { 0 };
    // device-64 byte = 0b00_RR_GG_BB → the area-gradation bit of each 2-bit channel.
    let bit = |byte: u8, ch_shift: u32| (((byte >> ch_shift) >> shift) & 1) as u32;
    let (re, ge, be) = (bit(even, 4), bit(even, 2), bit(even, 0)); // even x → R0/G0/B0
    let (ro, go, bo) = (bit(odd, 4), bit(odd, 2), bit(odd, 0)); // odd  x → R1/G1/B1
    re | (ro << 1) | (ge << 2) | (go << 3) | (be << 4) | (bo << 5)
}

/// Pack one **row** of device-64 ([`FbDevice64`](crate::FbDevice64)) pixels into the LS021 FLPR
/// write-buffer words: the **MSB sub-line** into `out[0..BCK_PER_SUBLINE]`, the **LSB sub-line**
/// into `out[BCK_PER_SUBLINE..ROW_WORDS]`. `row` is one framebuffer row — [`WIDTH`] device-64
/// bytes (`0b00_RR_GG_BB`, the [`PackDevice64`](crate::framebuffer::PackDevice64) format). The 4
/// trailing dummy columns of each sub-line are black.
///
/// Panics if `row.len() < WIDTH` or `out.len() < ROW_WORDS` — a buffer-wiring bug, caught loudly
/// (this feeds bring-up firmware). The output words are written but **not** fenced/published; the
/// caller owns the cross-core barrier + the "buffer ready" handshake.
pub fn pack_row(row: &[u8], out: &mut [u32]) {
    assert!(row.len() >= WIDTH, "row shorter than the panel width");
    assert!(out.len() >= ROW_WORDS, "out shorter than a full row buffer");
    for col in 0..COLS_PER_SUBLINE {
        let even = row[2 * col]; // even x → R0/G0/B0
        let odd = row[2 * col + 1]; // odd  x → R1/G1/B1
        out[col] = pack_pair(even, odd, true); // MSB sub-line
        out[BCK_PER_SUBLINE + col] = pack_pair(even, odd, false); // LSB sub-line
    }
    // Trailing dummy/flush columns of both sub-lines = black.
    for col in COLS_PER_SUBLINE..BCK_PER_SUBLINE {
        out[col] = 0;
        out[BCK_PER_SUBLINE + col] = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// device-64 byte from a `(r, g, b)` RGB222 level triple (`0..=3` each) — the
    /// [`PackDevice64`](crate::framebuffer::PackDevice64) `0b00_RR_GG_BB` layout.
    fn dev64(r: u8, g: u8, b: u8) -> u8 {
        (r << 4) | (g << 2) | b
    }

    /// Independent re-derivation of the golden reference word: this mirrors
    /// `PanelBus::plane_bits` + the `R0..B1` bit positions from `src/ls021.rs` (epic #139),
    /// written out longhand so the test fails if `pack_pair` ever drifts from it.
    fn golden_word(even: (u8, u8, u8), odd: (u8, u8, u8), msb: bool) -> u32 {
        // plane_bits: MSB plane = level>>1, LSB plane = level&1.
        let plane = |l: u8| if msb { (l >> 1) & 1 } else { l & 1 } as u32;
        let r0 = plane(even.0);
        let g0 = plane(even.1);
        let b0 = plane(even.2);
        let r1 = plane(odd.0);
        let g1 = plane(odd.1);
        let b1 = plane(odd.2);
        r0 | (r1 << 1) | (g0 << 2) | (g1 << 3) | (b0 << 4) | (b1 << 5)
    }

    fn empty_row() -> [u8; WIDTH] {
        [0u8; WIDTH]
    }

    #[test]
    fn black_row_is_all_zero() {
        let mut out = [0xAAAA_AAAAu32; ROW_WORDS];
        pack_row(&empty_row(), &mut out);
        assert!(out.iter().all(|&w| w == 0), "black row must pack to all-zero words");
    }

    /// Solid white: every data column is `0x3F` in both planes (all 6 lines high, odd == even),
    /// the 4 dummy columns are `0`. This is exactly the `pack_solid(3,3,3)` stand-in F3 used.
    #[test]
    fn solid_white_matches_pack_solid() {
        let row = [dev64(3, 3, 3); WIDTH];
        let mut out = [0u32; ROW_WORDS];
        pack_row(&row, &mut out);
        for col in 0..COLS_PER_SUBLINE {
            assert_eq!(out[col], 0x3F, "MSB data col {col}");
            assert_eq!(out[BCK_PER_SUBLINE + col], 0x3F, "LSB data col {col}");
        }
        for col in COLS_PER_SUBLINE..BCK_PER_SUBLINE {
            assert_eq!(out[col], 0, "MSB dummy col {col}");
            assert_eq!(out[BCK_PER_SUBLINE + col], 0, "LSB dummy col {col}");
        }
    }

    /// Pure channels at level 3 land on the right line pairs (catches an R/G/B swap): red lights
    /// only `R0|R1` (bits 0,1 → `0x03`), green only `G0|G1` (bits 2,3 → `0x0C`), blue only
    /// `B0|B1` (bits 4,5 → `0x30`). At full level both planes carry the bit.
    #[test]
    fn pure_channels_hit_the_right_lines() {
        for (level, mask) in [((3, 0, 0), 0x03u32), ((0, 3, 0), 0x0C), ((0, 0, 3), 0x30)] {
            let row = [dev64(level.0, level.1, level.2); WIDTH];
            let mut out = [0u32; ROW_WORDS];
            pack_row(&row, &mut out);
            assert_eq!(out[0], mask, "MSB plane for {level:?}");
            assert_eq!(out[BCK_PER_SUBLINE], mask, "LSB plane for {level:?}");
        }
    }

    /// The area-gradation split: level 2 is MSB-only (2/3 block), level 1 is LSB-only (1/3 block).
    /// A red `(2,0,0)` pixel ⇒ `R0|R1` set in the MSB plane, nothing in the LSB plane; `(1,0,0)`
    /// the reverse.
    #[test]
    fn mid_levels_split_across_planes() {
        let two = [dev64(2, 0, 0); WIDTH];
        let mut out = [0u32; ROW_WORDS];
        pack_row(&two, &mut out);
        assert_eq!(out[0], 0x03, "level 2 → MSB plane only");
        assert_eq!(out[BCK_PER_SUBLINE], 0x00, "level 2 → nothing in LSB plane");

        let one = [dev64(1, 0, 0); WIDTH];
        pack_row(&one, &mut out);
        assert_eq!(out[0], 0x00, "level 1 → nothing in MSB plane");
        assert_eq!(out[BCK_PER_SUBLINE], 0x03, "level 1 → LSB plane only");
    }

    /// Odd and even columns are distinct lines: a row whose even pixels are red and odd pixels are
    /// blue must put red only on `R0` (bit0) and blue only on `B1` (bit5) — proves no odd/even
    /// interleave error and that the pair maps `even→*0, odd→*1`.
    #[test]
    fn odd_even_interleave_is_distinct() {
        let mut row = empty_row();
        for (x, px) in row.iter_mut().enumerate() {
            *px = if x % 2 == 0 { dev64(3, 0, 0) } else { dev64(0, 0, 3) };
        }
        let mut out = [0u32; ROW_WORDS];
        pack_row(&row, &mut out);
        // R0 (bit0, even=red) and B1 (bit5, odd=blue) → 0b10_0001 = 0x21.
        assert_eq!(out[0], 0x21, "even red on R0, odd blue on B1");
    }

    /// Full-row agreement against the longhand golden re-derivation across an arbitrary spatial
    /// pattern (every pixel a different RGB222 value) for both planes — the catch-all that would
    /// flag any bit-position or plane drift `pack_pair` might pick up.
    #[test]
    fn matches_golden_reference_over_a_pattern() {
        let mut row = empty_row();
        let levels = |x: usize| {
            let r = (x % 4) as u8;
            let g = ((x / 4) % 4) as u8;
            let b = ((x / 16) % 4) as u8;
            (r, g, b)
        };
        for (x, px) in row.iter_mut().enumerate() {
            let (r, g, b) = levels(x);
            *px = dev64(r, g, b);
        }
        let mut out = [0u32; ROW_WORDS];
        pack_row(&row, &mut out);
        for col in 0..COLS_PER_SUBLINE {
            let even = levels(2 * col);
            let odd = levels(2 * col + 1);
            assert_eq!(out[col], golden_word(even, odd, true), "MSB col {col}");
            assert_eq!(out[BCK_PER_SUBLINE + col], golden_word(even, odd, false), "LSB col {col}");
        }
    }

    #[test]
    #[should_panic(expected = "row shorter than the panel width")]
    fn short_row_panics() {
        let mut out = [0u32; ROW_WORDS];
        pack_row(&[0u8; WIDTH - 1], &mut out);
    }

    #[test]
    #[should_panic(expected = "out shorter than a full row buffer")]
    fn short_out_panics() {
        let mut out = [0u32; ROW_WORDS - 1];
        pack_row(&empty_row(), &mut out);
    }
}
