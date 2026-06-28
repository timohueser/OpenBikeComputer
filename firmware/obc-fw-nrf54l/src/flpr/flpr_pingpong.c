/* flpr_pingpong.c — LS021 FLPR ping-pong frame blob (issues #154/#155, epic #149).
 *
 * **The epic's headline deliverable** (F4), reused unchanged by F5 (#155) behind the
 * `obc_platform::Panel` seam — F5 only ratchets the bit-bang timing toward the spec frame (see the
 * "F5 speed-tune" note on the delay constants). Successor to the F3 full-frame blob (flpr_frame.c, #153):
 * keeps the *complete* LS021 waveform F3 built — the gate scan (`GSP`/`GCK`/`GEN`), the frame
 * envelope (`INTB`), and the F2 inner data-shift loop (`drive_subline`) — and changes exactly one
 * thing: where each pixel row's source words come from. F3 reused a single solid-colour buffer for
 * all 320 rows; F4 **ping-pongs two row buffers** (`buf[0]` = even rows, `buf[1]` = odd rows) so the
 * M33 packs row N+1 from the real RGB222 framebuffer into one buffer while the FLPR scans row N out
 * of the other. That makes the source spatially-varying (the 64-colour palette, the shapes card)
 * without ever needing more than two row buffers in shared RAM.
 *
 * Still ported line-for-line from the analyzer-verified M33 `PanelBus` (`src/ls021.rs`, epic #139) —
 * the golden reference. The two hard-won protocol rules from #143 carry over verbatim:
 *   • **`INTB` HIGH for the whole frame** — `INTB` low means "no write" (the panel holds its image),
 *     so every frame, the init-black one included, is enveloped in `INTB` high.
 *   • **`GCK` *level* selects the area block on the SAME gate line** — one pixel row = one `GCK`
 *     period: the **MSB plane** shifts in the `GCK`-HIGH phase (latched into the 2/3-area block) and
 *     the **LSB plane** in the `GCK`-LOW phase (the 1/3-area block), a `GEN` pulse latching each.
 *     There is no MSB/LSB select pin; `GCK` level is it. 320 gate lines, one gate advance per row.
 *
 * The M33 launcher is src/bin/ls021_flpr_bringup.rs; the spec (buffer format, pin/port map, the
 * ping-pong handshake, the LA recipe) is firmware/docs/ls021-flpr.md.
 *
 * The round-trip, per frame:
 *   M33   resets both buf[i] ready/consumed, pre-packs row 0 → buf[0] and row 1 → buf[1] from the
 *         framebuffer (each a ROW buffer: MSB sub-line [0..124) then LSB sub-line [124..248)), bumps
 *         each buf[i].ready, then writes cmd = CMD_RUN_FRAME + bumps m33_seq (seq last), dsb.
 *   FLPR  polls m33_seq; on a change, for CMD_RUN_FRAME runs one frame:
 *         INTB high → GSP → 2 dummy advances → 320 rows, each draining buf[row & 1] under the
 *         **ping-pong handshake** (wait ready != consumed, scan the gate row, set consumed = ready)
 *         → 6 dummy advances → INTB low. Meanwhile the M33 packs rows 2..319 into whichever buffer
 *         the FLPR just freed. The FLPR then bumps frame_count, writes status = rows scanned +
 *         flpr_seq = m33_seq (seq last), pokes EGU20 to fire the M33's EGU20 IRQ #201, and blinks
 *         LED0 (P2.09) once as a by-eye "drained a frame" marker.
 *   M33   wakes on the EGU20 IRQ, checks status == 320 && flpr_seq == m33_seq.
 *
 * **Two ports (the F2 lesson, now load-bearing).** The timing-critical bus — the 6 data lines +
 * `BCK` — is all on **P2** (the FLPR's fast trace domain), so the hot per-column loop stays single-
 * port. The **gate lines (`GSP`/`GCK`/`GEN`/`INTB`) and `BSP` are all on P1**; F2 already proved the
 * FLPR can drive a P1 GPIO (`BSP`), which the whole gate scan here depends on. COM (`VCOM`/`VB`/`VA`,
 * P2.07/08/10) free-runs on the **M33** the entire time — this is the first stage where both cores
 * drive the shared **P2** port at once, which is exactly why every GPIO touch below is a single
 * atomic OUTSET/OUTCLR of *this core's* pin mask (never a read-modify-write of OUT): the M33's COM
 * set/clears and the FLPR's source set/clears on disjoint pins never corrupt each other.
 *
 * Freestanding (see start.S + flpr.ld): no libc/libgcc, integer ops + a fence only — no CSRs, so
 * no rv32 multilib and no Zicsr.
 */

#include <stdint.h>

/* ── GPIO, secure aliases (the all-secure nrf54l15-app-s build F1 already drives). Each OUTSET/
 * OUTCLR is one atomic write, no read-modify-write of OUT. Offsets: OUT +0x00, OUTSET +0x04,
 * OUTCLR +0x08 (nRF54L15 PAC). ──
 *   P2 base 0x5005_0400 — source data + BCK + LED0 (the FLPR's fast trace domain).
 *   P1 base 0x500D_8200 — BSP + the gate lines GSP/GCK/GEN/INTB (all the slow, µs-scale signals). */
#define GPIO2_OUTSET (*(volatile uint32_t *)0x50050404u)
#define GPIO2_OUTCLR (*(volatile uint32_t *)0x50050408u)
#define GPIO1_OUTSET (*(volatile uint32_t *)0x500D8204u)
#define GPIO1_OUTCLR (*(volatile uint32_t *)0x500D8208u)

/* Source-bus pin masks on P2. Bit position = P2 pin index (the harness map in ls021-bringup.md):
 * R0=P2.00 G0=P2.02 B0=P2.04 are the **odd** pixel, R1=P2.01 G1=P2.03 B1=P2.05 the **even** pixel.
 * A write-buffer word holds those 6 data bits already shifted to these positions (DATA_MASK), so
 * presenting a column is `OUTCLR the zeros, OUTSET the ones` — no bit-twiddling on the RISC-V side. */
#define DATA_MASK 0x3Fu        /* P2.00..05 = R0,R1,G0,G1,B0,B1 (the 6 source data lines) */
#define BCK_MASK  (1u << 6)    /* P2.06 = BCK (source/shift clock) */
#define LED0_MASK (1u << 9)    /* P2.09 = on-board LED0 — by-eye "drained a frame" marker */

/* Gate + frame pin masks on P1 (bit position = P1 pin index; same harness map). All µs-scale, so
 * P1 is fine — P2 is reserved for the fast bus. BSP is the F2 P1 line; F3 adds the four gate lines. */
#define BSP_MASK  (1u << 7)    /* P1.07 = BSP  (sub-line start pulse) */
#define GSP_MASK  (1u << 11)   /* P1.11 = GSP  (gate start pulse, once per frame) */
#define GCK_MASK  (1u << 12)   /* P1.12 = GCK  (gate clock — HIGH = MSB/2-3 phase, LOW = LSB/1-3 phase) */
#define GEN_MASK  (1u << 4)    /* P1.04 = GEN  (gate output enable — latches the GCK-level-selected block) */
#define INTB_MASK (1u << 6)    /* P1.06 = INTB (frame envelope — HIGH for the whole frame write) */

/* FLPR → M33 doorbell via EGU20 (secure 0x500C_9000) — see ls021-flpr.md. */
#define EGU20_TRIGGER0 (*(volatile uint32_t *)0x500C9000u)

/* ── Shared control block at the SHARED-page base 0x2003_F000 (see flpr.ld / memory.x). Layout is
 * normative and identical to the Rust `Control` in ls021_flpr_bringup.rs — keep them in sync
 * (firmware/docs/ls021-flpr.md). All fields u32, little-endian. ── */
typedef struct {
    uint32_t ptr;      /* row-buffer base: MSB sub-line at [0..len), LSB sub-line at [len..2·len) */
    uint32_t len;      /* words per sub-line = BCK per sub-line (124); a row is 2·len words */
    uint32_t ready;    /* M33 set when filled — a token the FLPR echoes into `consumed` */
    uint32_t consumed; /* FLPR set when drained (= the serviced `ready` token) */
} buf_desc_t;          /* 16 bytes */

typedef struct {
    volatile uint32_t magic;       /* 0x00 M33: layout/version tag, checked before acting */
    volatile uint32_t m33_seq;     /* 0x04 M33: command sequence counter (the doorbell) */
    volatile uint32_t cmd;         /* 0x08 M33: command word (F3: a CMD_* code) */
    volatile uint32_t flpr_seq;    /* 0x0C FLPR: echoes the m33_seq it serviced (round-trip proof) */
    volatile uint32_t status;      /* 0x10 FLPR: ack/result (F3: rows scanned; boot: FLPR_ALIVE) */
    volatile uint32_t frame_count; /* 0x14 FLPR: frames drained (bumped per CMD_RUN_FRAME) */
    volatile buf_desc_t buf[2];    /* 0x18, 0x28 row-buffer descriptors — buf[0] even rows, buf[1] odd (ping-pong) */
    volatile uint32_t reserved[2]; /* 0x38 pad — forward-compat headroom */
} flpr_control_t;

/* Lock the cross-language contract: the Rust `Control` in ls021_flpr_bringup.rs asserts the same. */
_Static_assert(sizeof(flpr_control_t) == 64, "control block must be 64 bytes (matches M33 Control)");

#define CTRL ((volatile flpr_control_t *)0x2003F000u)

#define LAYOUT_MAGIC 0xF1C00001u /* "F1 control block" — must match the M33 */
#define FLPR_ALIVE   0x0000A11Eu /* boot confirmation */
#define FLPR_BADMAG  0x0BADCAFEu /* booted but the control-block magic mismatched */

/* Command codes (M33 → FLPR via `cmd`). One piece of work: run a full frame (F4 ping-ponging the
 * two row buffers; the F2 CMD_SHIFT_SUBLINE=1 is subsumed, the F3 single-buffer reuse generalised). */
#define CMD_RUN_FRAME 0x00000002u

/* ── Frame geometry (matches `PanelBus` in src/ls021.rs / the datasheet §6-5/§6-6 charts). ── */
#define COLS_PER_SUBLINE 120u /* 240 columns ÷ 2 pixels-per-BCK */
#define BCK_PER_SUBLINE  124u /* 120 data + 4 trailing dummy/flush BCK per sub-line */
#define ROWS_PER_FRAME   320u /* visible pixel rows = gate advances; each row carries BOTH area planes */
#define GATE_DUMMY_LEAD  2u   /* pipeline-fill dummy gate advances before the 320 data rows */
#define GATE_DUMMY_TRAIL 6u   /* "necessary signal" blank dummy advances after them */

/* Full memory fence: cross-core data ordering + a compiler barrier, so a guard field is never
 * observed before the data it guards (the M33 side uses dsb for the same contract). */
static inline void fence(void)
{
    __asm__ volatile("fence" ::: "memory");
}

/* ── Bit-bang delays (busy-loops). The FLPR clock is unconfigured at this stage, so these are
 * **LA-calibrated on the bench**, exactly like the M33 path's asm::delay counts. F2 measured
 * `busy(120) ≈ 9.4 µs` on the unconfigured (~64 MHz) FLPR ⇒ **~13 iters/µs**; the gate-scan delays
 * are derived from that the way the M33 path derives its from COUNTS_PER_US, so each clears its
 * datasheet minimum with margin.
 *
 * ## F5 speed-tune (issue #155) — what is safe to lower, and how far
 *
 * F2–F4 ran deliberately **bring-up-slow** (`BCK_HALF_ITERS = 120` ≈ 9.4 µs half ⇒ ~53 kHz BCK,
 * ~16× under the panel's 0.758 MHz max) so the analyzer resolved every edge. F5 ratchets toward the
 * spec ~53 ms frame. The **source shift is ~77 % of the frame** (124 BCK × 2 sub-lines × 320 rows),
 * so `BCK_HALF_ITERS` / `DATA_SETUP_ITERS` are the dominant — and the *only correctness-free* — lever:
 *   - These are now set **near the panel's BCK ceiling** (`BCK_HALF_ITERS = 8` ⇒ ~0.5–0.6 MHz BCK,
 *     under the 0.758 MHz max). **LA-verify on the bench:** confirm `BCK ≤ 0.758 MHz` with clean
 *     edges and the data lines settled before each rise. There is margin to go lower still
 *     (`BCK_HALF_ITERS = 5` ≈ 0.7 MHz) if the edges stay clean; back off if they don't.
 *   - The gate timings (`GCK_SETTLE`/`GEN_SETUP`/`GEN_HIGH`) are panel **electrical minimums**
 *     (GCK↔GEN setup/hold ≥16.37 µs, GEN valid ≥24.56 µs). **Do NOT lower below their µs values** —
 *     they are correctness, not slack. (`gen_pulse` drops its *leading* setup busy — the long data
 *     shift before it already supplies the GCK↔GEN setup — and keeps the GEN-high + trailing hold.)
 *
 * ## The bit-bang floor (why this won't hit ~53 ms without more)
 *
 * This driver is **sequential** (gate scan *then* source shift, per row) and bit-bangs every edge.
 * The FLPR already runs at the **full PLL clock** (the M33 boots it with `ClockSpeed::CK128`; there
 * is no separate VPR clock divider on this part — the only clock controls are the global HFXO/PLL —
 * so the FLPR is already maxed, *not* the ~64 MHz the early F2 busy-loop estimate suggested). Two
 * hard floors remain even with BCK at its ceiling:
 *   - **Source at max BCK** — 124 BCK × 2 sub-lines × 320 rows ÷ 0.758 MHz ≈ **105 ms**, irreducible
 *     without driving the bus faster than the panel allows.
 *   - **Gate minimums** — 2 `GEN` pulses/row at the spec mins ≈ 116 µs/row × 320 ≈ **37 ms**.
 *   - **GPIO/loop overhead** — ~4 GPIO writes + loop control per BCK column, a fixed ~0.6 ms/row that
 *     does *not* shrink with the delay counts.
 * So the sequential bit-bang floors around **~150 ms** at the BCK ceiling, not 53 ms. The ~53 ms spec
 * assumes a **pipelined** controller that overlaps the source shift with the gate scan. Getting there
 * needs a structural change, not a smaller busy count: **drive the source bus from hardware** (a
 * SPIM/SPI peripheral clocking `BCK` + the data lines by DMA) so the CPU runs the gate scan
 * *concurrently* with the shift — collapsing the ~105 ms source onto the ~37 ms gate. That is the same
 * machinery the deferred partial/dirty-line epic wants; tracked as an F5 follow-up. `push_frame` logs
 * the measured frame time each push — tune against that. ── */
#define ITERS_PER_US      13u /* bench calibration: busy(120) ≈ 9.4 µs on the unconfigured FLPR */
#define BCK_HALF_ITERS    4u                    /* each BCK phase — pushing toward the 0.758 MHz ceiling; LA-verify the actual BCK and back off if over */
#define DATA_SETUP_ITERS  3u                    /* source data stable before the BCK rising edge (spec ~335 ns) */
#define GCK_SETTLE_ITERS  (5u * ITERS_PER_US)   /* settle after a GCK level change before shifting */
#define GEN_SETUP_ITERS   (17u * ITERS_PER_US)  /* GCK↔GEN setup AND hold (spec ≥16.37 µs) */
#define GEN_HIGH_ITERS    (25u * ITERS_PER_US)  /* GEN valid-output window (spec ≥24.56 µs) */
#define GCK_HIGH_ITERS    (10u * ITERS_PER_US)  /* GCK high width for a DUMMY advance only */
#define FRAME_SETUP_ITERS (10u * ITERS_PER_US)  /* INTB→GSP and GSP→first GCK framing setup */

static void busy(uint32_t iters)
{
    for (volatile uint32_t i = 0; i < iters; i++) {
    }
}

/* Drain one source sub-line from `base` (a byte address into the row buffer): pulse BSP, then `len`
 * BCK presenting each word's 6 data bits on P2.00..05. Unchanged from F2 (`PanelBus::shift_subline_
 * with` timing): BSP high envelopes BCK(1) (released on the first BCK rising edge), data is set up
 * DATA_SETUP before each BCK rise, data lines left Lo after. The caller has already set GCK to this
 * plane's level — this touches only the source bus, never GCK/GEN. */
static void drive_subline(uint32_t base, uint32_t len)
{
    const volatile uint32_t *buf = (const volatile uint32_t *)(uintptr_t)base;

    GPIO1_OUTSET = BSP_MASK; /* BSP high — start of the sub-line (the chart's BSP envelope) */
    for (uint32_t col = 0; col < len; col++) {
        uint32_t data = buf[col] & DATA_MASK;
        GPIO2_OUTCLR = (~data) & DATA_MASK; /* lower the 0 data bits (P2.00..05 only) */
        GPIO2_OUTSET = data;                /* raise the 1 data bits — column now presented */
        busy(DATA_SETUP_ITERS);             /* data stable before BCK rises (spec ~335 ns) */

        GPIO2_OUTSET = BCK_MASK;            /* BCK rising edge latches this pixel-pair into the SR */
        if (col == 0) {
            GPIO1_OUTCLR = BSP_MASK;        /* BCK(1) rose within BSP high — now release BSP */
        }
        busy(BCK_HALF_ITERS);
        GPIO2_OUTCLR = BCK_MASK;            /* BCK low */
        busy(BCK_HALF_ITERS);
    }
    GPIO2_OUTCLR = DATA_MASK; /* leave the data lines Lo (boot-safe) after the sub-line */
}

/* Pulse GEN to latch the just-shifted sub-line into the **currently-selected** gate line. The caller
 * has set the GCK level first and that level chooses the target block: GCK HIGH → the 2/3-area (MSB)
 * cells, GCK LOW → the 1/3-area (LSB) cells. Fired clear of the GCK edges (GCK↔GEN setup/hold
 * ≥16.37 µs). The **GCK↔GEN setup is already supplied by the long data shift** that ran before this
 * call (hundreds of µs of GCK-stable time ≫ 16.37 µs), so the *leading* setup busy is redundant and
 * dropped (an F5 speed-tune saving ~11 ms/frame); only the GEN-high window and the *trailing* hold
 * (GEN-low → the next GCK edge, which follows immediately) remain. Mirrors `PanelBus::gen_pulse`. */
static void gen_pulse(void)
{
    GPIO1_OUTSET = GEN_MASK;
    busy(GEN_HIGH_ITERS);  /* valid-output window ≥24.56 µs */
    GPIO1_OUTCLR = GEN_MASK;
    busy(GEN_SETUP_ITERS); /* GEN → next GCK edge hold ≥16.37 µs */
}

/* One DUMMY gate advance: a clean GCK period (high→low) with GEN/BCK idle — the pipeline-fill /
 * "necessary signal" blank rows bracketing the 320 data rows. `release_gsp` drops GSP on the rising
 * edge (the very first advance of a frame, so GSP high overlaps GCK(1) per the chart). Mirrors
 * `PanelBus::dummy_advance`. */
static void dummy_advance(int release_gsp)
{
    GPIO1_OUTSET = GCK_MASK;
    if (release_gsp) {
        GPIO1_OUTCLR = GSP_MASK; /* GCK(1) rising edge within GSP high — release GSP */
    }
    busy(GCK_HIGH_ITERS);
    GPIO1_OUTCLR = GCK_MASK;
    busy(GCK_SETTLE_ITERS);
}

/* Write **one pixel row** = one gate line carrying both area planes, one GCK period. `base`/`len`
 * are the row buffer: MSB sub-line at [base .. base+len words), LSB sub-line at [base+len .. +len).
 *   • MSB phase — raise GCK (this rising edge advances the gate to this row), shift the MSB sub-line,
 *     GEN → latches the 2/3-area cells while GCK is HIGH;
 *   • LSB phase — drop GCK (same gate line, NOT an advance), shift the LSB sub-line, GEN → latches
 *     the 1/3-area cells while GCK is LOW.
 * The advance to the next row is the GCK rising edge that opens the next call's MSB phase, so there
 * is exactly one gate advance per pixel row. Mirrors `PanelBus::write_gate_line`. */
static void write_gate_row(uint32_t base, uint32_t len)
{
    /* ── MSB phase: GCK HIGH selects the 2/3-area block; this rising edge advances the gate ── */
    GPIO1_OUTSET = GCK_MASK;
    busy(GCK_SETTLE_ITERS);
    drive_subline(base, len);
    gen_pulse(); /* latch 2/3-area cells — GCK still HIGH */

    /* ── LSB phase: GCK LOW selects the 1/3-area block; SAME gate line, no advance ── */
    GPIO1_OUTCLR = GCK_MASK;
    busy(GCK_SETTLE_ITERS);
    drive_subline(base + len * 4u, len); /* LSB sub-line = the next `len` words (×4 = byte offset) */
    gen_pulse();                         /* latch 1/3-area cells — GCK now LOW */
}

/* Drain one row buffer under the **ping-pong handshake**, then scan its gate row. `i` selects
 * buf[i] (buf[0] = even rows, buf[1] = odd). The two per-buffer counters are the doorbell:
 *   • the M33 fills buf[i] from the framebuffer, then bumps `ready` (data first, dsb, then ready);
 *   • the FLPR waits until `ready != consumed` (a fresh row is published), scans it, then sets
 *     `consumed = ready` (the row is drained → the M33 may refill buf[i]).
 * So the M33 never overwrites a buffer the FLPR is mid-scan on, and the FLPR never scans a
 * half-filled one. At bring-up BCK the M33 (µs/row) races far ahead of the FLPR (ms/row), so the
 * wait is virtually always already satisfied — it is backpressure insurance. The wait sits *before*
 * the gate row, with GCK still LOW from the previous row's LSB phase, so even a (never-observed)
 * stall just holds the inter-row gap with INTB high and no GEN — benign, nothing latches. */
static void drain_row(uint32_t i)
{
    while (CTRL->buf[i].ready == CTRL->buf[i].consumed) {
        /* spin for the M33's "row ready" — dedicated core, polling is correct (the F1 channel) */
    }
    fence(); /* `ready` seen before we read the buffer words it guards */
    write_gate_row(CTRL->buf[i].ptr, CTRL->buf[i].len);
    CTRL->buf[i].consumed = CTRL->buf[i].ready; /* echo the ready token = "drained, buffer free" */
    fence();                                    /* `consumed` visible before the M33 refills buf[i] */
}

/* Run one full frame, ping-ponging the two row buffers. The datasheet frame envelope is unchanged
 * from F3 (mirroring `PanelBus::fill_solid`); the only difference is that each of the 320 pixel rows
 * is drained from buf[row & 1] under the handshake (`drain_row`) instead of reusing one buffer:
 *   1. INTB high for the WHOLE frame (every frame, init-black included — INTB low = "no write").
 *   2. GSP start pulse, then 320 pixel rows (each one gate line in one GCK period), bracketed by
 *      lead/trail dummy gate advances; GSP releases on the first lead dummy's GCK edge.
 * Returns the number of rows scanned (== ROWS_PER_FRAME), which the M33 checks. */
static uint32_t run_frame(void)
{
    GPIO1_OUTSET = INTB_MASK; /* frame envelope HIGH for the whole write */
    busy(FRAME_SETUP_ITERS);  /* thsINTB: INTB stable before GSP */
    GPIO1_OUTSET = GSP_MASK;  /* start pulse: loads the first gate */
    busy(FRAME_SETUP_ITERS);  /* thsGSP: GSP stable before the first GCK */

    for (uint32_t i = 0; i < GATE_DUMMY_LEAD; i++) {
        dummy_advance(i == 0); /* GSP releases on the very first GCK edge */
    }
    for (uint32_t row = 0; row < ROWS_PER_FRAME; row++) {
        drain_row(row & 1u); /* even rows ← buf[0], odd rows ← buf[1] */
    }
    for (uint32_t i = 0; i < GATE_DUMMY_TRAIL; i++) {
        dummy_advance(0);
    }

    GPIO1_OUTCLR = GSP_MASK;  /* belt-and-suspenders; already released on the first lead dummy */
    GPIO1_OUTCLR = INTB_MASK; /* end of frame — the panel now holds the image */
    return ROWS_PER_FRAME;
}

void flpr_main(void)
{
    /* Boot handshake: confirm the control block layout, then stamp ALIVE (the F0/F1 boot proof). */
    if (CTRL->magic != LAYOUT_MAGIC) {
        CTRL->status = FLPR_BADMAG;
        for (;;) {
        }
    }
    CTRL->flpr_seq = 0;
    CTRL->status = FLPR_ALIVE;
    fence();

    uint32_t last_seq = 0;
    for (;;) {
        /* M33→FLPR command doorbell: poll the shared-RAM sequence (M33 pre-fills both buffers + cmd
         * then m33_seq+dsb). The FLPR is a dedicated core, so polling is correct. This starts a
         * frame; the per-row ping-pong then runs on the separate buf[i] ready/consumed handshake. */
        uint32_t seq = CTRL->m33_seq;
        if (seq == last_seq) {
            continue;
        }
        fence(); /* sequence seen before we read the command + buffer it guards */
        uint32_t cmd = CTRL->cmd;
        last_seq = seq;

        uint32_t rows = 0;
        if (cmd == CMD_RUN_FRAME) {
            /* Scan a frame, draining buf[row & 1] under the ping-pong handshake (the per-buffer
             * ready/consumed echo now happens per row inside drain_row, not once per frame). */
            rows = run_frame();
            CTRL->frame_count++; /* frames drained (liveness + the round-trip contract) */
        }

        CTRL->status = rows;  /* rows scanned (0 for an unknown cmd) — the M33 cross-checks == 320 */
        fence();              /* status/consumed visible before the seq guard */
        CTRL->flpr_seq = seq; /* seq last = the ack guard the M33 reads */
        fence();              /* ack visible before we ring the doorbell */

        EGU20_TRIGGER0 = 1u; /* ring the M33: EGU20.EVENTS_TRIGGERED[0] -> M33 EGU20 IRQ (#201) */

        /* By-eye liveness marker, *after* the ack and the captured waveform so it perturbs neither:
         * one LED0 blink per drained frame. */
        GPIO2_OUTSET = LED0_MASK;
        busy(200000u);
        GPIO2_OUTCLR = LED0_MASK;
    }
}
