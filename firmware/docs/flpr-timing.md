# FLPR display timing evidence

This is the measured basis for the timing constants in
`obc-fw-nrf54l/src/flpr/flpr_scan.c`. The source file owns the policy beside the constants.

## Calibration

On 2026-07-04, one nRF54L DK at 128 MHz measured `busy()` through an M33→FLPR command/ack round
trip. The two-microsecond transport floor was negligible against 80–2,300 ms samples.

- one loop iteration: **80.5 ns** (12.4 iterations/µs);
- call overhead: approximately **102 ns**;
- build: RV32EMC blob at `-Os` for calibration, later `-O2` for the accepted image.

This calibrates software delay cost, not GPIO pulse width. Electrical claims still require a logic
analyzer or equivalent capture.

## Accepted result

The optimized image presents a full 320-row frame in **44.1 ms**, down from the 95.9 ms pre-pass
baseline. Representative partial pushes measured on the same panel were 9.43 ms for 64 rows,
12.3 ms for 88 rows and 15.7 ms for 112 rows.

Gate timings remain above the panel minimums. The DDR source bus deliberately relies on pack/GPIO
work for an approximately 210 ns BCK half, below the 660 ns datasheet minimum. That choice was
visually clean on one room-temperature panel; fully in-spec pacing was costed at roughly 80 ms per
full frame and was not selected.

This is a single-unit result, not a production tolerance claim. A new panel revision, clock,
compiler or hot-loop change must remeasure full/partial frame times and check solids, map colors,
fine contours, column doubling, sparkle and banding on glass. If a unit fails, add explicit pacing
after each present rather than trimming the gate minima.
