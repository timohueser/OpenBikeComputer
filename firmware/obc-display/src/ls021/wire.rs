//! LS021B7DD02 source-bus wire pack: the host-tested RGB222 to panel-wire transform the FLPR
//! backend drains. The area-gradation split, the odd/even column interleave and the pre-shift to
//! GPIO bit positions live here, so the FLPR side stays a dumb `store → pulse BCK` loop.
//!
//! The panel writes each pixel row as two area planes selected by the gate clock's level, an MSB
//! plane (the 2/3-area block) and an LSB plane (the 1/3-area block), each shifted in as one
//! sub-line of [`BCK_PER_SUBLINE`] words. One row packs to [`ROW_WORDS`] u32s, MSB sub-line first.
//!
//! One word is one pixel pair: the even-`x` pixel on the `*0` lines and the odd-`x` pixel on the
//! `*1` lines, with the 6 data bits already shifted to their P2 GPIO positions (`DATA_MASK`), so
//! the FLPR presents a column with one `OUTCLR` and one `OUTSET`. `BCK` is the FLPR's own pulse,
//! not part of the word, and the 4 trailing dummy columns are black.
//!
//! The positions are sparse because the six sEMMC card pads own `P2.00–05`: the display lines live
//! on the four pins the SD-SPI path freed plus two pads time-shared with the card, whose `CTRLSEL`
//! hands them to the display blob or the sEMMC peripheral per mode. The two never run at once.
//!
//! | line | `R0` | `R1` | `G0` | `G1` | `B0` | `B1` |
//! |---|---|---|---|---|---|---|
//! | P2 pin | `.06` | `.08` | `.09` | `.10` | `.00` (`D3`) | `.04` (`D1`) |
//! | word bit | 6 | 8 | 9 | 10 | 0 | 4 |
//!
//! This module is the normative definition of that layout, and the FLPR's C port
//! (`obc-fw-nrf54l/src/flpr/flpr_scan.c`) mirrors it bit for bit.
//!
//! The panel is DDR: it latches the source bus on both `BCK` edges, so the FLPR drains one word
//! per edge and clocks the 120 pairs out in about 60 cycles. The pack is edge-agnostic.
//!
//! Area-gradation split: each channel's level is 2 bits, so the MSB plane carries `level >> 1` and
//! the LSB plane `level & 1`. A logic analyzer proved this split, and the tests re-derive it.

/// Panel width in pixels, clocked as 120 pixel pairs per sub-line.
pub const WIDTH: usize = 240;
/// Data columns clocked per sub-line.
pub const COLS_PER_SUBLINE: usize = WIDTH / 2;
/// `BCK` words per sub-line: 120 data columns plus 4 dummy columns that push the last pixels
/// through the source shift register. Also the FLPR's per-sub-line `len`.
pub const BCK_PER_SUBLINE: usize = COLS_PER_SUBLINE + 4;
/// Words in one full row buffer: the MSB sub-line followed by the LSB sub-line.
pub const ROW_WORDS: usize = 2 * BCK_PER_SUBLINE;

/// Pack one pixel pair (two device-64 bytes, `0b00_RR_GG_BB`) into one source-bus word for the
/// given area plane. `even` is the even-`x` pixel on `R0/G0/B0`, `odd` the odd-`x` pixel on
/// `R1/G1/B1`. `msb` selects the area-gradation bit.
#[inline]
fn pack_pair(even: u8, odd: u8, msb: bool) -> u32 {
    let shift = if msb { 1 } else { 0 };
    // device-64 byte = 0b00_RR_GG_BB → the area-gradation bit of each 2-bit channel.
    let bit = |byte: u8, ch_shift: u32| (((byte >> ch_shift) >> shift) & 1) as u32;
    let (re, ge, be) = (bit(even, 4), bit(even, 2), bit(even, 0)); // even x → R0/G0/B0
    let (ro, go, bo) = (bit(odd, 4), bit(odd, 2), bit(odd, 0)); // odd  x → R1/G1/B1

    // Shifted straight to the P2 pin positions of the bus; see the module doc's pin table.
    (re << 6) | (ro << 8) | (ge << 9) | (go << 10) | be | (bo << 4)
}

/// Pack one row of device-64 pixels into the FLPR write-buffer words: the MSB sub-line into
/// `out[0..BCK_PER_SUBLINE]` and the LSB sub-line after it. The 4 trailing dummy columns are
/// black.
///
/// Panics if a buffer is too small, which is a wiring bug. The words are written but not fenced:
/// the caller owns the cross-core barrier and the buffer-ready handshake.
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
    extern crate std;

    use std::string::String;
    use std::vec::Vec;

    use super::*;

    /// device-64 byte from an `(r, g, b)` RGB222 level triple (`0..=3` each).
    fn dev64(r: u8, g: u8, b: u8) -> u8 {
        (r << 4) | (g << 2) | b
    }

    /// The source-bus pin map, written as P2 pin indexes rather than word shifts: a packed word's
    /// bit position is the pin index, so re-deriving the goldens catches a slip in `pack_pair`.
    const R0_PIN: u32 = 6; // P2.06 (was SD-SPI SCK)
    const R1_PIN: u32 = 8; // P2.08 (was SD-SPI MOSI)
    const G0_PIN: u32 = 9; // P2.09 (was SD-SPI MISO)
    const G1_PIN: u32 = 10; // P2.10 (was SD-SPI CS)
    const B0_PIN: u32 = 0; // P2.00, time-shared with sEMMC D3
    const B1_PIN: u32 = 4; // P2.04, time-shared with sEMMC D1

    /// The six data lines together: the FLPR's mask, and a solid-white column's packed word.
    const DATA_MASK: u32 =
        (1 << R0_PIN) | (1 << R1_PIN) | (1 << G0_PIN) | (1 << G1_PIN) | (1 << B0_PIN) | (1 << B1_PIN);
    /// Per-channel line pairs (`*0` even + `*1` odd), derived from the pin map above.
    const RED_LINES: u32 = (1 << R0_PIN) | (1 << R1_PIN);
    const GREEN_LINES: u32 = (1 << G0_PIN) | (1 << G1_PIN);
    const BLUE_LINES: u32 = (1 << B0_PIN) | (1 << B1_PIN);

    /// Cross-language pin: the mask this module packs to must be the literal the FLPR blob clears
    /// and sets. A failure means the two have diverged and the panel would show garbage.
    #[test]
    fn data_mask_matches_the_flpr_blob() {
        assert_eq!(DATA_MASK, 0x751, "DATA_MASK must equal flpr_scan.c's 0x751");
        assert_eq!(RED_LINES, 0x140);
        assert_eq!(GREEN_LINES, 0x600);
        assert_eq!(BLUE_LINES, 0x011);
    }

    // Nothing else in CI pins the C blob, and an edit that keeps `DATA_MASK` but permutes
    // `pack_word`'s shifts scrambles the panel while passing the whole host suite, because the
    // two are independent encodings of one layout. So read the C source at compile time and assert
    // its mask and its six shift amounts against the pin map. The parse is strict on structure: a
    // missing define, a reworded return or a moved file fails the test loudly.

    /// The FLPR scan blob's C source, embedded at test-compile time, under `cfg(test)`.
    const FLPR_SCAN_C: &str = include_str!("../../../obc-fw-nrf54l/src/flpr/flpr_scan.c");

    /// `#define DATA_MASK 0x751u` to `0x751`. Fails the test if the define is gone or malformed.
    fn c_data_mask(src: &str) -> u32 {
        let line = src
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with("#define DATA_MASK"))
            .expect("flpr_scan.c: no `#define DATA_MASK` line — did the blob move or get reworded?");
        let tok: String =
            line["#define DATA_MASK".len()..].trim_start().chars().take_while(char::is_ascii_alphanumeric).collect();
        let hex = tok
            .strip_prefix("0x")
            .or_else(|| tok.strip_prefix("0X"))
            .unwrap_or_else(|| panic!("flpr_scan.c: DATA_MASK `{tok}` is not a 0x-prefixed literal"))
            .trim_end_matches(['u', 'U']);
        u32::from_str_radix(hex, 16)
            .unwrap_or_else(|e| panic!("flpr_scan.c: DATA_MASK `{tok}` is not a hex literal: {e}"))
    }

    /// The `name << shift` terms of `pack_word`'s return expression, in source order; a bare term
    /// is shift 0. Fails the test unless there are exactly six single-variable terms.
    fn c_pack_word_shifts(src: &str) -> Vec<(String, u32)> {
        let fn_at = src
            .find("uint32_t pack_word(")
            .expect("flpr_scan.c: `pack_word` not found — did the blob move or get renamed?");
        let ret_at =
            fn_at + src[fn_at..].find("return ").expect("flpr_scan.c: `pack_word` has no `return` — reworded?");
        let end =
            ret_at + src[ret_at..].find(';').expect("flpr_scan.c: `pack_word`'s return statement is unterminated");
        let expr = &src[ret_at + "return ".len()..end];

        let terms: Vec<(String, u32)> = expr
            .split('|')
            .map(|term| {
                let t: String = term.chars().filter(|c| !c.is_whitespace() && *c != '(' && *c != ')').collect();
                match t.split_once("<<") {
                    None => (t, 0),
                    Some((name, shift)) => {
                        let parsed = shift.trim_end_matches(['u', 'U']).parse::<u32>().unwrap_or_else(|e| {
                            panic!("flpr_scan.c: pack_word term `{t}` has a non-numeric shift: {e}")
                        });
                        (name.into(), parsed)
                    }
                }
            })
            .collect();
        assert_eq!(
            terms.len(),
            6,
            "flpr_scan.c: pack_word's return should OR exactly 6 terms (one per data line), got {terms:?}"
        );
        terms
    }

    /// The blob and this module must encode the same pin map, mask and per-line positions alike:
    /// the only thing between a permuted `pack_word` and a scrambled panel.
    #[test]
    fn flpr_blob_carries_the_same_pin_map() {
        assert_eq!(c_data_mask(FLPR_SCAN_C), DATA_MASK, "flpr_scan.c's DATA_MASK disagrees with this module's pin map");

        // `pack_word`'s locals: `*e` = the even pixel (the `*0` lines), `*o` = the odd (`*1`).
        let expected = [("re", R0_PIN), ("ro", R1_PIN), ("ge", G0_PIN), ("go", G1_PIN), ("be", B0_PIN), ("bo", B1_PIN)];
        let got = c_pack_word_shifts(FLPR_SCAN_C);
        for (name, pin) in expected {
            let (_, shift) = got
                .iter()
                .find(|(n, _)| n.as_str() == name)
                .unwrap_or_else(|| panic!("flpr_scan.c: pack_word's return has no `{name}` term (parsed: {got:?})"));
            assert_eq!(*shift, pin, "flpr_scan.c: `{name}` is shifted to bit {shift}, the pin map says {pin}");
        }
    }

    /// Independent longhand re-derivation of the golden reference word, so the test fails if
    /// `pack_pair` drifts.
    fn golden_word(even: (u8, u8, u8), odd: (u8, u8, u8), msb: bool) -> u32 {
        // plane_bits: MSB plane = level>>1, LSB plane = level&1.
        let plane = |l: u8| if msb { (l >> 1) & 1 } else { l & 1 } as u32;
        let r0 = plane(even.0);
        let g0 = plane(even.1);
        let b0 = plane(even.2);
        let r1 = plane(odd.0);
        let g1 = plane(odd.1);
        let b1 = plane(odd.2);
        (r0 << R0_PIN) | (r1 << R1_PIN) | (g0 << G0_PIN) | (g1 << G1_PIN) | (b0 << B0_PIN) | (b1 << B1_PIN)
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

    /// Solid white: every data column is `DATA_MASK` in both planes and the dummy columns are `0`.
    /// It also proves the pack never sets a bit outside the six data lines.
    #[test]
    fn solid_white_matches_pack_solid() {
        let row = [dev64(3, 3, 3); WIDTH];
        let mut out = [0u32; ROW_WORDS];
        pack_row(&row, &mut out);
        for col in 0..COLS_PER_SUBLINE {
            assert_eq!(out[col], DATA_MASK, "MSB data col {col}");
            assert_eq!(out[BCK_PER_SUBLINE + col], DATA_MASK, "LSB data col {col}");
        }
        for col in COLS_PER_SUBLINE..BCK_PER_SUBLINE {
            assert_eq!(out[col], 0, "MSB dummy col {col}");
            assert_eq!(out[BCK_PER_SUBLINE + col], 0, "LSB dummy col {col}");
        }
    }

    /// Pure channels at level 3 land on the right line pairs, which catches an R/G/B swap. At full
    /// level both planes carry the bit.
    #[test]
    fn pure_channels_hit_the_right_lines() {
        for (level, mask) in [((3, 0, 0), RED_LINES), ((0, 3, 0), GREEN_LINES), ((0, 0, 3), BLUE_LINES)] {
            let row = [dev64(level.0, level.1, level.2); WIDTH];
            let mut out = [0u32; ROW_WORDS];
            pack_row(&row, &mut out);
            assert_eq!(out[0], mask, "MSB plane for {level:?}");
            assert_eq!(out[BCK_PER_SUBLINE], mask, "LSB plane for {level:?}");
        }
    }

    /// The area-gradation split: level 2 is MSB-only and level 1 is LSB-only.
    #[test]
    fn mid_levels_split_across_planes() {
        let two = [dev64(2, 0, 0); WIDTH];
        let mut out = [0u32; ROW_WORDS];
        pack_row(&two, &mut out);
        assert_eq!(out[0], RED_LINES, "level 2 → MSB plane only");
        assert_eq!(out[BCK_PER_SUBLINE], 0x00, "level 2 → nothing in LSB plane");

        let one = [dev64(1, 0, 0); WIDTH];
        pack_row(&one, &mut out);
        assert_eq!(out[0], 0x00, "level 1 → nothing in MSB plane");
        assert_eq!(out[BCK_PER_SUBLINE], RED_LINES, "level 1 → LSB plane only");
    }

    /// Odd and even columns are distinct lines, so a row of red even pixels and blue odd pixels
    /// must light only `R0` and `B1`: no interleave error, and the pair maps even to `*0`.
    #[test]
    fn odd_even_interleave_is_distinct() {
        let mut row = empty_row();
        for (x, px) in row.iter_mut().enumerate() {
            *px = if x % 2 == 0 { dev64(3, 0, 0) } else { dev64(0, 0, 3) };
        }
        let mut out = [0u32; ROW_WORDS];
        pack_row(&row, &mut out);
        // R0 (bit6, even=red) and B1 (bit4, odd=blue) → 0x40 | 0x10 = 0x50.
        assert_eq!(out[0], (1 << R0_PIN) | (1 << B1_PIN), "even red on R0, odd blue on B1");
        assert_eq!(out[0], 0x50);
    }

    /// The pin map, one line at a time: light one channel of one parity, and the packed word must
    /// be exactly that line's bit. The tightest pin on the map, because the pattern test below
    /// compares only pairs, where a same-channel swap can hide. Each case also asserts the literal
    /// word, hand-computed from the pin numbers, because the derived form would survive a joint
    /// swap of the shift and the pin constant.
    #[test]
    fn each_line_is_addressable_on_its_own() {
        // (even pixel RGB, odd pixel RGB, the line's bit, that bit written out longhand)
        let cases = [
            ((3, 0, 0), (0, 0, 0), 1u32 << R0_PIN, 0x040u32), // R0 = P2.06
            ((0, 0, 0), (3, 0, 0), 1 << R1_PIN, 0x100),       // R1 = P2.08
            ((0, 3, 0), (0, 0, 0), 1 << G0_PIN, 0x200),       // G0 = P2.09
            ((0, 0, 0), (0, 3, 0), 1 << G1_PIN, 0x400),       // G1 = P2.10
            ((0, 0, 3), (0, 0, 0), 1 << B0_PIN, 0x001),       // B0 = P2.00
            ((0, 0, 0), (0, 0, 3), 1 << B1_PIN, 0x010),       // B1 = P2.04
        ];
        for (even, odd, line, literal) in cases {
            assert_eq!(line, literal, "pin map drifted: even {even:?} / odd {odd:?}");
            let mut row = empty_row();
            for (x, px) in row.iter_mut().enumerate() {
                let (r, g, b) = if x % 2 == 0 { even } else { odd };
                *px = dev64(r, g, b);
            }
            let mut out = [0u32; ROW_WORDS];
            pack_row(&row, &mut out);
            // Level 3 sets the channel's bit in both area planes and nothing else anywhere.
            assert_eq!(out[0], literal, "MSB plane: even {even:?} / odd {odd:?} is not one-hot");
            assert_eq!(out[BCK_PER_SUBLINE], literal, "LSB plane: even {even:?} / odd {odd:?} is not one-hot");
        }
    }

    /// Full-row agreement against the longhand re-derivation over an arbitrary pattern in both
    /// planes: the catch-all for any bit-position or plane drift. Every channel changes level
    /// between neighbouring pixels, so an odd/even swap shows up here.
    #[test]
    fn matches_golden_reference_over_a_pattern() {
        let mut row = empty_row();
        let levels = |x: usize| {
            let r = ((x + 1) % 4) as u8;
            let g = ((3 * x + 2) % 4) as u8;
            let b = ((7 * x) % 4) as u8;
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
