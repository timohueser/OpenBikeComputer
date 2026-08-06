//! **LS021B7DD02 source-bus wire pack** — the host-tested RGB222 → panel-wire transform the FLPR
//! backend drains (epic #149).
//!
//! The sibling of [`device64_to_rgb565`](crate::device64_to_rgb565): where that expands a device-64
//! ([`FbDevice64`](crate::FbDevice64)) byte back to RGB565 (for the overlay-window fill and the
//! simulator), this packs a whole **row**
//! of device-64 bytes into the LS021's parallel **source bus** wire words — the format the FLPR
//! clocks out over `BSP`/`BCK` + the 6 data lines (`R0/G0/B0`, `R1/G1/B1`). The trickiest bits (the
//! area-gradation split, the odd/even column interleave, the pre-shift to GPIO bit positions) live
//! here, unit-tested against a longhand re-derivation of the analyzer-verified protocol, so the
//! FLPR side stays a dumb `store → pulse BCK` loop.
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
//! shifted to their P2 GPIO positions — `bit6 R0, bit8 R1, bit9 G0, bit10 G1, bit0 B0, bit4 B1`
//! (`= DATA_MASK 0x751`) — so the FLPR presents a column with one `OUTCLR (~w & 0x751)` + one
//! `OUTSET (w & 0x751)` and no bit-twiddling. `BCK` is *not* in the word; it is the FLPR's own pulse.
//! The 4 trailing dummy/flush columns of each sub-line are black (`0`).
//!
//! The positions are **sparse** because the six fixed sEMMC card pads own `P2.00–05` since the
//! storage pivot (issue #1158): the display data lines live on the four pins the retired SD-SPI
//! path freed plus the two pads time-shared with the card's `D3`/`D1`, whose `CTRLSEL` hands them
//! to the display blob or to the sEMMC soft peripheral per mode (the two never run at once).
//!
//! | line | `R0` | `R1` | `G0` | `G1` | `B0` | `B1` |
//! |---|---|---|---|---|---|---|
//! | P2 pin | `.06` | `.08` | `.09` | `.10` | `.00` (`D3`) | `.04` (`D1`) |
//! | word bit | 6 | 8 | 9 | 10 | 0 | 4 |
//!
//! This module is the **normative** definition of that layout; the FLPR's C port
//! (`obc-fw-nrf54l/src/flpr/flpr_scan.c`, `pack_word` + `DATA_MASK`) mirrors it bit-for-bit and
//! the two always move in the same commit — the goldens below pin the host side.
//!
//! **The panel is DDR**: it latches the source bus on *both* `BCK` edges, so the FLPR drains these
//! words **one per edge** — word `2k` before the rising edge, `2k+1` before the falling — clocking
//! the 120 pairs out in ~60 `BCK` cycles. The pack is edge-agnostic (it lays the pairs out in
//! order); the rising/falling split lives in the FLPR's `drive_subline`.
//!
//! ## Area-gradation split
//!
//! Each channel's device-64 level is 2 bits (`0..=3`): the **MSB plane** carries the high bit
//! (`level >> 1`, the 2/3-area block), the **LSB plane** the low bit (`level & 1`, the 1/3-area
//! block). This is the split the retired M33 bit-bang driver proved on a logic analyzer (epic
//! #139, driver deleted in #176 — the FLPR blob inherited the protocol); the test module
//! re-derives it longhand and asserts byte-for-byte agreement.

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
/// given area plane. `even` is the even-`x` pixel → `R0/G0/B0` (word bits 6/9/0); `odd` is the
/// odd-`x` pixel → `R1/G1/B1` (word bits 8/10/4). `msb` selects the area-gradation bit
/// (`level >> 1` for the 2/3-area MSB plane, `level & 1` for the 1/3-area LSB plane).
#[inline]
fn pack_pair(even: u8, odd: u8, msb: bool) -> u32 {
    let shift = if msb { 1 } else { 0 };
    // device-64 byte = 0b00_RR_GG_BB → the area-gradation bit of each 2-bit channel.
    let bit = |byte: u8, ch_shift: u32| (((byte >> ch_shift) >> shift) & 1) as u32;
    let (re, ge, be) = (bit(even, 4), bit(even, 2), bit(even, 0)); // even x → R0/G0/B0
    let (ro, go, bo) = (bit(odd, 4), bit(odd, 2), bit(odd, 0)); // odd  x → R1/G1/B1

    // Shifted straight to the P2 pin positions of the rehomed bus (module doc's pin table):
    // R0→P2.06, R1→P2.08, G0→P2.09, G1→P2.10, B0→P2.00, B1→P2.04.
    (re << 6) | (ro << 8) | (ge << 9) | (go << 10) | be | (bo << 4)
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
    extern crate std;

    use std::string::String;
    use std::vec::Vec;

    use super::*;

    /// device-64 byte from a `(r, g, b)` RGB222 level triple (`0..=3` each) — the
    /// [`PackDevice64`](crate::framebuffer::PackDevice64) `0b00_RR_GG_BB` layout.
    fn dev64(r: u8, g: u8, b: u8) -> u8 {
        (r << 4) | (g << 2) | b
    }

    /// The rehomed source-bus **pin map** (issue #1158), written as P2 pin indexes rather than as
    /// word shifts: a packed word's bit position *is* the pin index, so re-deriving the goldens
    /// from these catches a bit-position slip in `pack_pair` instead of mirroring it.
    const R0_PIN: u32 = 6; // P2.06 (was SD-SPI SCK)
    const R1_PIN: u32 = 8; // P2.08 (was SD-SPI MOSI)
    const G0_PIN: u32 = 9; // P2.09 (was SD-SPI MISO)
    const G1_PIN: u32 = 10; // P2.10 (was SD-SPI CS)
    const B0_PIN: u32 = 0; // P2.00, time-shared with sEMMC D3
    const B1_PIN: u32 = 4; // P2.04, time-shared with sEMMC D1

    /// The six data lines together — the FLPR's `OUTCLR`/`OUTSET` mask, `DATA_MASK` in
    /// `obc-fw-nrf54l/src/flpr/flpr_scan.c`. Also the packed word of a solid-white column.
    const DATA_MASK: u32 =
        (1 << R0_PIN) | (1 << R1_PIN) | (1 << G0_PIN) | (1 << G1_PIN) | (1 << B0_PIN) | (1 << B1_PIN);
    /// Per-channel line pairs (`*0` even + `*1` odd), derived from the pin map above.
    const RED_LINES: u32 = (1 << R0_PIN) | (1 << R1_PIN);
    const GREEN_LINES: u32 = (1 << G0_PIN) | (1 << G1_PIN);
    const BLUE_LINES: u32 = (1 << B0_PIN) | (1 << B1_PIN);

    /// Cross-language pin: the mask this module packs to must be the literal the FLPR blob clears
    /// and sets. If this fails, `flpr_scan.c`'s `DATA_MASK` and the pin map above have diverged —
    /// the panel would show garbage on glass.
    #[test]
    fn data_mask_matches_the_flpr_blob() {
        assert_eq!(DATA_MASK, 0x751, "DATA_MASK must equal flpr_scan.c's 0x751");
        assert_eq!(RED_LINES, 0x140);
        assert_eq!(GREEN_LINES, 0x600);
        assert_eq!(BLUE_LINES, 0x011);
    }

    // ── Cross-language drift guard (added by the #1159 review) ────────────────────────────────
    //
    // Nothing else in CI pins the C blob. An edit to `flpr_scan.c` that keeps `DATA_MASK 0x751`
    // but permutes `pack_word`'s shifts (say `re << 8` / `ro << 6`) scrambles the panel and passes
    // 100 % of the host suite, because the host pack and the blob are two independent encodings of
    // the same layout. So: read the C source at compile time and assert its `DATA_MASK` and its six
    // `pack_word` shift amounts against the pin map above.
    //
    // The parse is tolerant of whitespace and parentheses but **strict on structure** — a missing
    // define, a missing `pack_word`, a reworded return expression, or a moved file all make this
    // test FAIL loudly rather than silently pass on nothing. It deliberately does not model which
    // device-64 channel feeds which variable. The proper long-term home for the whole M33↔FLPR
    // contract is build.rs's single-definition mechanism (issue #346, which already emits
    // `flpr_contract.h`); folding the pin map in there is a follow-up.

    /// The FLPR scan blob's C source, embedded at test-compile time. `obc-display` is `publish =
    /// false`, so reaching across the crate boundary here costs nothing at package time, and this
    /// is a `cfg(test)` item — no device build ever sees it.
    const FLPR_SCAN_C: &str = include_str!("../../../obc-fw-nrf54l/src/flpr/flpr_scan.c");

    /// `#define DATA_MASK 0x751u` → `0x751`. Panics (= test failure) if the define is gone or is
    /// not a `0x`-prefixed literal.
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

    /// The `name << shift` terms of `pack_word`'s return expression, in source order. A bare term
    /// (`be`) is shift 0. Panics (= test failure) unless there are exactly six single-variable
    /// terms.
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

    /// The blob and this module must encode the *same* pin map — mask **and** per-line positions.
    /// This is the only thing standing between a permuted `pack_word` and a scrambled panel.
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

    /// Independent longhand re-derivation of the golden reference word (the analyzer-verified
    /// plane split + the `R0..B1` pin map above), so the test fails if `pack_pair` drifts.
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

    /// Solid white: every data column is `DATA_MASK` (`0x751`) in both planes (all 6 lines high,
    /// odd == even), the 4 dummy columns are `0`. This is exactly the `pack_solid(3,3,3)` stand-in
    /// F3 used. It also proves the pack never sets a bit outside the six data lines — anything
    /// stray would be an `OUTSET` on a pin the FLPR does not own (BCK, or an sEMMC card pad).
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

    /// Pure channels at level 3 land on the right line pairs (catches an R/G/B swap): red lights
    /// only `R0|R1` (bits 6,8 → `0x140`), green only `G0|G1` (bits 9,10 → `0x600`), blue only
    /// `B0|B1` (bits 0,4 → `0x011`). At full level both planes carry the bit.
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

    /// The area-gradation split: level 2 is MSB-only (2/3 block), level 1 is LSB-only (1/3 block).
    /// A red `(2,0,0)` pixel ⇒ `R0|R1` set in the MSB plane, nothing in the LSB plane; `(1,0,0)`
    /// the reverse.
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

    /// Odd and even columns are distinct lines: a row whose even pixels are red and odd pixels are
    /// blue must put red only on `R0` (bit6 = P2.06) and blue only on `B1` (bit4 = P2.04) —
    /// proves no odd/even interleave error and that the pair maps `even→*0, odd→*1`.
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

    /// **The pin map, one line at a time.** Light exactly one channel of exactly one parity and the
    /// packed word must be exactly that line's bit — six one-hot assertions covering all six pins.
    /// This is the tightest pin on the rehomed map: the pattern test below only compares *pairs*,
    /// so a `G0`↔`G1` (or `B0`↔`B1`) swap can hide there whenever the two pixels of a pair happen
    /// to carry the same level; here it cannot.
    ///
    /// Each case also asserts the **literal** word (#1159 review): the derived `1 << *_PIN` form
    /// alone would survive a *joint* swap that moves `pack_pair`'s shift and the pin constant
    /// together — `DATA_MASK`/`GREEN_LINES` only pin the unordered set `{9, 10}`. The literals are
    /// hand-computed from the pin numbers, so nothing in this file can be edited into agreement
    /// with a wrong panel wiring.
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

    /// Full-row agreement against the longhand golden re-derivation across an arbitrary spatial
    /// pattern for both planes — the catch-all that would flag any bit-position or plane drift
    /// `pack_pair` might pick up. Every channel changes level between *neighbouring* pixels (the
    /// multipliers are coprime with 4), so an odd/even swap on any line shows up here too.
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
