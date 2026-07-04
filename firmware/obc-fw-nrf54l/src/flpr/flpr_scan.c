/* flpr_scan.c — LS021 FLPR direct-framebuffer scan blob (issue #347, epic #353).
 *
 * Successor to the F4/F5 ping-pong blob (flpr_pingpong.c, issues #154/#155): keeps the *complete*
 * LS021 waveform — the gate scan (`GSP`/`GCK`/`GEN`), the frame envelope (`INTB`), the DDR inner
 * data-shift loop — and changes exactly one thing: **where the wire words come from**. The
 * ping-pong predated the resident framebuffer: the M33 packed each row into two shared buffers and
 * both cores ran a per-row ready/consumed handshake. Today the resident device-64 framebuffer is a
 * stable byte-per-pixel plane in shared SRAM, so this blob reads it **directly** (`fb_addr` in the
 * control block, stride = 240 bytes/row) and packs the wire words itself — the per-row handshake,
 * both row buffers, and the M33's whole-frame busy-poll packing loop are gone. The M33's present
 * becomes "publish spans, ring the doorbell, await the EGU20 ack".
 *
 * **The pack is a line-for-line port of `obc-platform/src/ls021_wire.rs`** (`pack_pair` /
 * `pack_row`) — the host-tested normative reference; its test module (golden-reference words,
 * odd/even interleave, area-gradation split) is the spec for `pack_word` below. Wire behavior is
 * byte-identical to the M33-packed path.
 *
 * Still ported from the analyzer-verified M33 `PanelBus` driver (epic #139; the bit-bang driver
 * itself was retired in #176, the protocol it proved is the golden reference). The two hard-won
 * protocol rules from #143 carry over verbatim:
 *   • **`INTB` HIGH for the whole frame** — `INTB` low means "no write" (the panel holds its image),
 *     so every frame, the init-black one included, is enveloped in `INTB` high.
 *   • **`GCK` *level* selects the area block on the SAME gate line** — one pixel row = one `GCK`
 *     period: the **MSB plane** shifts in the `GCK`-HIGH phase (latched into the 2/3-area block) and
 *     the **LSB plane** in the `GCK`-LOW phase (the 1/3-area block), a `GEN` pulse latching each.
 *     There is no MSB/LSB select pin; `GCK` level is it. 320 gate lines, one gate advance per row.
 *
 * **Partial / dirty-row updates (issue #163)** are unchanged: the scan is driven by a dirty-row
 * span list (`n_spans` + `spans[]`, packed `(start_row << 16) | count`, ascending + disjoint): it
 * fast-forwards the gate over clean rows (`dummy_advance`, GEN idle — nothing latches, the row
 * keeps its memory), writes only the spanned rows, and **stops early** after the last one. A full
 * frame is the degenerate `n_spans=1, spans[0]=(0,320)`.
 *
 * The round-trip, per frame (contract v2 — see `Control` in ls021_flpr.rs):
 *   M33   publishes the span list + `fb_addr`, writes cmd = CMD_RUN_FRAME, dsb, bumps m33_seq
 *         (seq last), then **awaits the EGU20 ack** (or times out).
 *   FLPR  polls m33_seq; on a change, for CMD_RUN_FRAME runs the span-masked scan straight out of
 *         the framebuffer: INTB high → GSP → lead dummies → per span {fast-forward to start, then
 *         pack+shift each row from `fb_addr + row*240`} → trail dummies → INTB low. Then bumps
 *         frame_count, writes status = dirty rows scanned + flpr_seq = m33_seq (seq last, fenced),
 *         and pokes EGU20 → the M33's EGU20 IRQ wakes the awaiting present.
 *
 * **Two ports (the F2 lesson, still load-bearing).** The timing-critical bus — the 6 data lines +
 * `BCK` — is all on **P2** (the FLPR's fast trace domain); the gate lines (`GSP`/`GCK`/`GEN`/
 * `INTB`) and `BSP` are on P1. COM (`VCOM`/`VB`/`VA`, P2.07/08/10) free-runs on the **M33** the
 * entire time — both cores drive the shared **P2** port at once, which is exactly why every GPIO
 * touch below is a single atomic OUTSET/OUTCLR of *this core's* pin mask (never a read-modify-write
 * of OUT): the M33's COM set/clears and the FLPR's source set/clears on disjoint pins never corrupt
 * each other.
 *
 * Freestanding (see start.S + the generated flpr.ld): no libc/libgcc, integer ops + a fence only —
 * no CSRs, so no rv32 multilib and no Zicsr.
 */

#include <stdint.h>

/* The M33<->FLPR cross-core contract — the control-block address, layout magic / status stamps,
 * command codes, and the span cap — generated into $OUT_DIR by build.rs (its `contract` module is
 * the single definition site, issue #346); the blob compile passes -I $OUT_DIR. */
#include "flpr_contract.h"

/* ── GPIO, secure aliases (the all-secure nrf54l15-app-s build F1 already drives). Each OUTSET/
 * OUTCLR is one atomic write, no read-modify-write of OUT. Offsets: OUT +0x00, OUTSET +0x04,
 * OUTCLR +0x08 (nRF54L15 PAC). ──
 *   P2 base 0x5005_0400 — source data + BCK (the FLPR's fast trace domain).
 *   P1 base 0x500D_8200 — BSP + the gate lines GSP/GCK/GEN/INTB (all the slow, µs-scale signals). */
#define GPIO2_OUTSET (*(volatile uint32_t *)0x50050404u)
#define GPIO2_OUTCLR (*(volatile uint32_t *)0x50050408u)
#define GPIO1_OUTSET (*(volatile uint32_t *)0x500D8204u)
#define GPIO1_OUTCLR (*(volatile uint32_t *)0x500D8208u)

/* Source-bus pin masks on P2. Bit position = P2 pin index (the harness map in ls021-bringup.md):
 * R0=P2.00 G0=P2.02 B0=P2.04 are the **even** pixel, R1=P2.01 G1=P2.03 B1=P2.05 the **odd** pixel.
 * `pack_word` below packs a pixel pair's 6 data bits already shifted to these positions
 * (DATA_MASK), so presenting a column is `OUTCLR the zeros, OUTSET the ones`. */
#define DATA_MASK 0x3Fu        /* P2.00..05 = R0,R1,G0,G1,B0,B1 (the 6 source data lines) */
#define BCK_MASK  (1u << 6)    /* P2.06 = BCK (source/shift clock) */

/* Gate + frame pin masks on P1 (bit position = P1 pin index). All µs-scale, so P1 is fine — P2 is
 * reserved for the fast bus.
 *
 * ⚠️ **These masks MUST stay in lock-step with the M33 `Output::new` pins in `main.rs`** — if a
 * pin is not broken out on your DK, remap it here *and* there (issue #165 moved them off the
 * SD/VCOM pins the bring-up bench borrowed). */
#define BSP_MASK  (1u << 14)   /* P1.14 = BSP  (sub-line start pulse) */
#define GSP_MASK  (1u << 0)    /* P1.00 = GSP  (gate start pulse, once per frame) */
#define GCK_MASK  (1u << 1)    /* P1.01 = GCK  (gate clock — HIGH = MSB/2-3 phase, LOW = LSB/1-3 phase) */
#define GEN_MASK  (1u << 12)   /* P1.12 = GEN  (gate output enable — latches the GCK-level-selected block) */
#define INTB_MASK (1u << 10)   /* P1.10 = INTB (frame envelope — HIGH for the whole frame write; LED1) */

/* FLPR → M33 doorbell via EGU20 (secure 0x500C_9000) — the M33 arms EGU20's TRIGGERED[0] IRQ and
 * awaits it as the frame ack (issue #347; previously written but unarmed). */
#define EGU20_TRIGGER0 (*(volatile uint32_t *)0x500C9000u)

/* ── Shared control block at the SHARED-page base FLPR_CONTROL_ADDR (see the generated flpr.ld /
 * memory.x). **Contract v2** (issue #347): the ping-pong `buf[2]` descriptors are gone; `fb_addr`
 * points the scan at the resident framebuffer instead. Layout is normative and identical to the
 * Rust `Control` in ls021_flpr.rs — keep them in sync (firmware/docs/ls021-flpr.md). All fields
 * u32, little-endian. ── */
typedef struct {
    volatile uint32_t magic;       /* 0x00 M33: layout/version tag, checked before acting */
    volatile uint32_t m33_seq;     /* 0x04 M33: command sequence counter (the doorbell) */
    volatile uint32_t cmd;         /* 0x08 M33: command word (a CMD_* code) */
    volatile uint32_t flpr_seq;    /* 0x0C FLPR: echoes the m33_seq it serviced (round-trip proof) */
    volatile uint32_t status;      /* 0x10 FLPR: ack/result (dirty rows scanned; boot: FLPR_ALIVE) */
    volatile uint32_t frame_count; /* 0x14 FLPR: frames drained (bumped per CMD_RUN_FRAME) */
    volatile uint32_t fb_addr;     /* 0x18 M33: resident device-64 framebuffer base (stride 240 B/row) */
    volatile uint32_t n_spans;     /* 0x1C M33: #dirty-row spans in `spans` (1 = a full frame `(0,320)`) */
    volatile uint32_t spans[MAX_DIRTY_SPANS]; /* 0x20 M33: packed `(start_row << 16) | count`, ascending + disjoint */
} flpr_control_t;                  /* 0x60 = 96 bytes */

/* Lock the cross-language contract: the Rust `Control` in ls021_flpr.rs asserts the same 96 bytes
 * (the two structs alias the same shared-RAM bytes, so the size must match exactly). */
_Static_assert(sizeof(flpr_control_t) == 96, "control block must be 96 bytes (matches the M33 Control, contract v2)");

#define CTRL ((volatile flpr_control_t *)FLPR_CONTROL_ADDR)

/* ── Frame geometry (the datasheet §6-5/§6-6 charts; mirrors obc_platform::ls021_wire). ── */
#define COLS_PER_SUBLINE 120u /* 240 columns ÷ 2 pixels-per-BCK-edge */
#define BCK_PER_SUBLINE  124u /* 120 data + 4 trailing dummy/flush BCK words per sub-line */
#define ROW_STRIDE       (2u * COLS_PER_SUBLINE) /* framebuffer bytes per row (byte per pixel) */
#define ROWS_PER_FRAME   320u /* visible pixel rows = gate advances; each row carries BOTH area planes */
#define GATE_DUMMY_LEAD  2u   /* pipeline-fill dummy gate advances before the 320 data rows */
#define GATE_DUMMY_TRAIL 6u   /* "necessary signal" blank dummy advances after them */

/* Full memory fence: cross-core data ordering + a compiler barrier, so a guard field is never
 * observed before the data it guards (the M33 side uses dsb for the same contract). */
static inline void fence(void)
{
    __asm__ volatile("fence" ::: "memory");
}

/* ── Bit-bang delays (busy-loops), LA-calibrated on the bench: F2 measured `busy(120) ≈ 9.4 µs`
 * ⇒ **~13 iters/µs**. The DDR drive (F5) latches the source bus on BOTH BCK edges, so each half
 * must satisfy the panel spec independently: BCK width ≥660 ns high AND low (`thwBCK`/`tlwBCK`),
 * data set-up ≥335 ns before *each* edge (`tsRGB`). The bench unit runs happily over-spec (the
 * constants below); the in-spec question is issue #348's measured decision.
 *
 * ## Direct-fb pack rides inside the delay windows (issue #347)
 *
 * `drive_subline` now packs each wire word from the framebuffer itself (~20 RV32 integer ops,
 * ≈1–2 busy-iters' worth). The pack of the *next* word runs **inside the current word's data-setup
 * window** — after the data lines are presented, before the BCK edge — where the loop previously
 * just busy-spun. `DATA_SETUP_TOPUP_ITERS` is the busy() remainder that tops the window back up to
 * the old `DATA_SETUP_ITERS = 3` total (pack ≈ 2 iters + top-up 1 ≈ the old 3):
 *   - colours sparkle / wrong on glass ⇒ the pack is running shorter than budgeted on your unit —
 *     raise the top-up (2 or 3 restores the old margin at a small frame-time cost);
 *   - the frame-time log regressing vs the ~97 ms baseline ⇒ lower it / tune in #348's LA pass.
 *
 * The gate timings (`GCK_SETTLE`/`GEN_SETUP`/`GEN_HIGH`) are panel **electrical minimums**
 * (GCK↔GEN setup/hold ≥16.37 µs, GEN valid ≥24.56 µs). **Do NOT lower below their µs values** —
 * they are correctness, not slack. (`gen_pulse` drops its *leading* setup busy — the long data
 * shift before it already supplies the GCK↔GEN setup — and keeps the GEN-high + trailing hold.) ── */
#define ITERS_PER_US      13u /* bench calibration: busy(120) ≈ 9.4 µs */
#define BCK_HALF_ITERS    2u                    /* each BCK half-period — ⚠️ OVER-SPEC bench value (~180 ns vs the ≥660 ns min; works on this unit) */
#define DATA_SETUP_TOPUP_ITERS 1u               /* busy() after the in-window pack; pack (~2 iters) + this ≈ the old DATA_SETUP_ITERS 3 (~280 ns, under the spec ~335 ns min — see the note above) */
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

/* Pack one source-bus wire word straight from the framebuffer row — the C port of
 * `obc_platform::ls021_wire::pack_pair` (the normative, host-tested reference; its tests are the
 * spec). Word `k` of a sub-line is the pixel pair `(row[2k], row[2k+1])`: the even-x pixel on the
 * `*0` lines (bits 0/2/4), the odd-x pixel on the `*1` lines (bits 1/3/5), pre-shifted to the P2
 * GPIO positions (DATA_MASK). `shift` selects the area-gradation bit of each 2-bit device-64
 * channel (`0b00_RR_GG_BB`): 1 = the MSB plane (level>>1, the 2/3-area block), 0 = the LSB plane
 * (level&1, the 1/3-area block). The 4 trailing dummy/flush columns (k ≥ 120) are black. */
static inline uint32_t pack_word(const volatile uint8_t *row, uint32_t k, uint32_t shift)
{
    if (k >= COLS_PER_SUBLINE) {
        return 0; /* trailing dummy/flush columns */
    }
    uint32_t even = row[2u * k];
    uint32_t odd = row[2u * k + 1u];
    uint32_t re = (even >> (4u + shift)) & 1u, ro = (odd >> (4u + shift)) & 1u;
    uint32_t ge = (even >> (2u + shift)) & 1u, go = (odd >> (2u + shift)) & 1u;
    uint32_t be = (even >> shift) & 1u, bo = (odd >> shift) & 1u;
    return re | (ro << 1) | (ge << 2) | (go << 3) | (be << 4) | (bo << 5);
}

/* Shift one source sub-line (one area plane of one pixel row) straight from the framebuffer:
 * pulse BSP, then clock the BCK_PER_SUBLINE wire words out over P2.00..05, packing each word from
 * `row` on the fly (`pack_word`).
 *
 * **DDR drive (F5, verified on glass)** — the panel latches the source bus on **BOTH BCK edges**:
 * word `2k` set up before the **rising** edge, word `2k+1` before the **falling** edge, one
 * distinct pair per edge, `len/2` BCK cycles for `len` words. The **pack of the next word runs
 * inside the current word's data-setup window** (see the delay note above): the data lines are
 * already presented and must merely stay stable until the edge, and the pack touches only
 * registers — so the mandatory setup wait does the packing work the M33 used to busy-poll for.
 *
 * BSP high envelopes the first rising edge (released on it). The caller has set GCK to this
 * plane's level — this touches only the source bus, never GCK/GEN. */
static void drive_subline(const volatile uint8_t *row, uint32_t shift)
{
    uint32_t w0 = pack_word(row, 0, shift); /* word 0 packed ahead of the sub-line */

    GPIO1_OUTSET = BSP_MASK; /* BSP high — start of the sub-line (the chart's BSP envelope) */
    for (uint32_t k = 0; k < BCK_PER_SUBLINE; k += 2) {
        /* ── rising-edge column: word k (already packed) ── */
        GPIO2_OUTCLR = (~w0) & DATA_MASK;
        GPIO2_OUTSET = w0;
        uint32_t w1 = pack_word(row, k + 1u, shift); /* pack word k+1 inside word k's setup window */
        busy(DATA_SETUP_TOPUP_ITERS);
        GPIO2_OUTSET = BCK_MASK; /* rising edge latches word k */
        if (k == 0) {
            GPIO1_OUTCLR = BSP_MASK; /* BCK(1) rose within BSP high — now release BSP */
        }
        busy(BCK_HALF_ITERS);

        /* ── falling-edge column: word k+1 ── */
        GPIO2_OUTCLR = (~w1) & DATA_MASK;
        GPIO2_OUTSET = w1;
        w0 = pack_word(row, k + 2u, shift); /* pack the next rising word inside this setup window */
        busy(DATA_SETUP_TOPUP_ITERS);
        GPIO2_OUTCLR = BCK_MASK; /* falling edge latches word k+1 */
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

/* Write **one pixel row** = one gate line carrying both area planes, one GCK period, straight from
 * the framebuffer row at `row` (240 device-64 bytes):
 *   • MSB phase — raise GCK (this rising edge advances the gate to this row), shift the MSB
 *     sub-line (`shift = 1`), GEN → latches the 2/3-area cells while GCK is HIGH;
 *   • LSB phase — drop GCK (same gate line, NOT an advance), shift the LSB sub-line (`shift = 0`),
 *     GEN → latches the 1/3-area cells while GCK is LOW.
 * The advance to the next row is the GCK rising edge that opens the next call's MSB phase, so there
 * is exactly one gate advance per pixel row. Mirrors `PanelBus::write_gate_line`. */
static void write_gate_row(const volatile uint8_t *row)
{
    /* ── MSB phase: GCK HIGH selects the 2/3-area block; this rising edge advances the gate ── */
    GPIO1_OUTSET = GCK_MASK;
    busy(GCK_SETTLE_ITERS);
    drive_subline(row, 1u);
    gen_pulse(); /* latch 2/3-area cells — GCK still HIGH */

    /* ── LSB phase: GCK LOW selects the 1/3-area block; SAME gate line, no advance ── */
    GPIO1_OUTCLR = GCK_MASK;
    busy(GCK_SETTLE_ITERS);
    drive_subline(row, 0u);
    gen_pulse(); /* latch 1/3-area cells — GCK now LOW */
}

/* Run one frame as a **span-masked scan** (issue #163) straight out of the framebuffer. The gate
 * token walks a shift register top-down — `GSP` loads it, every `GCK` period (a `dummy_advance` OR
 * a `write_gate_row`'s MSB-phase rising edge) advances it one row — so to reach visible row N you
 * must clock past 0..N-1, but you can:
 *   • **fast-forward a clean row** with `dummy_advance` (a `GCK` period, `GEN` idle) — it advances
 *     the gate but latches nothing, so that row keeps its retained memory; and
 *   • **stop early** — after the last dirty row + the trailing flush we drop `INTB`, leaving the
 *     gate token parked partway. Rows below the last dirty span are never advanced → they retain.
 * So a partial frame costs one cheap `GCK` advance per row from the top down to the lowest dirty
 * row, plus a full pack+shift per row actually changed; rows below cost nothing.
 *
 * Driven by the dirty-row descriptor (`CTRL->n_spans` + `CTRL->spans`, packed `(start<<16)|count`,
 * ascending + disjoint; `n_spans` clamped — never trust shared RAM) over the framebuffer at
 * `CTRL->fb_addr`. The envelope:
 *   1. INTB high for the WHOLE frame (every frame, init-black included — INTB low = "no write").
 *   2. GSP start pulse, lead dummy advances (GSP releases on the first's GCK edge).
 *   3. For each span: fast-forward to its start, then pack+write each of its rows from the fb.
 *   4. Trailing dummy flush, then INTB low (early stop — never a forced scan to row 320).
 * Returns the number of **dirty** rows scanned (= sum of span counts), which the M33 cross-checks. */
static uint32_t run_frame(void)
{
    GPIO1_OUTSET = INTB_MASK; /* frame envelope HIGH for the whole write */
    busy(FRAME_SETUP_ITERS);  /* thsINTB: INTB stable before GSP */
    GPIO1_OUTSET = GSP_MASK;  /* start pulse: loads the first gate */
    busy(FRAME_SETUP_ITERS);  /* thsGSP: GSP stable before the first GCK */

    for (uint32_t i = 0; i < GATE_DUMMY_LEAD; i++) {
        dummy_advance(i == 0); /* GSP releases on the very first GCK edge */
    }

    const volatile uint8_t *fb = (const volatile uint8_t *)(uintptr_t)CTRL->fb_addr;
    uint32_t n_spans = CTRL->n_spans;
    if (n_spans > MAX_DIRTY_SPANS) {
        n_spans = MAX_DIRTY_SPANS; /* distrust shared RAM: a corrupted count must not overrun spans[] */
    }
    uint32_t gate = 0;  /* next visible row the gate token will land on (0-based, post-lead-dummy) */
    uint32_t dirty = 0; /* total dirty rows written */
    for (uint32_t s = 0; s < n_spans; s++) {
        uint32_t span = CTRL->spans[s];
        uint32_t start = span >> 16;
        uint32_t count = span & 0xFFFFu;
        while (gate < start) {
            dummy_advance(0); /* fast-forward a clean row: advance the gate, GEN idle, latch nothing */
            gate++;
        }
        for (uint32_t k = 0; k < count; k++) {
            write_gate_row(fb + gate * ROW_STRIDE); /* its MSB-phase GCK edge advances onto `gate` */
            dirty++;
            gate++;
        }
    }

    for (uint32_t i = 0; i < GATE_DUMMY_TRAIL; i++) {
        dummy_advance(0); /* "necessary signal" blank flush after the last dirty row */
    }

    GPIO1_OUTCLR = GSP_MASK;  /* belt-and-suspenders; already released on the first lead dummy */
    GPIO1_OUTCLR = INTB_MASK; /* end of frame — the panel now holds the image (early-stopped) */
    return dirty;             /* total dirty rows scanned — the M33 cross-checks == sum of span counts */
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
        /* M33→FLPR command doorbell: poll the shared-RAM sequence (the M33 publishes fb_addr +
         * spans + cmd, dsb, then bumps m33_seq). The FLPR is a dedicated core, so polling is
         * correct. */
        uint32_t seq = CTRL->m33_seq;
        if (seq == last_seq) {
            continue;
        }
        fence(); /* sequence seen before we read the command + fb/spans it guards */
        uint32_t cmd = CTRL->cmd;
        last_seq = seq;

        uint32_t rows = 0;
        if (cmd == CMD_RUN_FRAME) {
            /* Run the span-masked scan (#163) straight out of the framebuffer (#347): fast-forward
             * clean rows, pack+write the dirty spans, early-stop after the last dirty row. */
            rows = run_frame();
            CTRL->frame_count++; /* frames drained (liveness + the round-trip contract) */
        }

        CTRL->status = rows;  /* dirty rows scanned (0 for an unknown cmd) — M33 cross-checks == sum(span counts) */
        fence();              /* status visible before the seq guard */
        CTRL->flpr_seq = seq; /* seq last = the ack guard the M33 reads */
        fence();              /* ack visible before we ring the doorbell */

        EGU20_TRIGGER0 = 1u; /* ring the M33: EGU20.EVENTS_TRIGGERED[0] → the armed M33 EGU20 IRQ (#347) */
    }
}
