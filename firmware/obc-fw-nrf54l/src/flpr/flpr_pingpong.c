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
 * **Partial / dirty-row updates (issue #163).** The scan is driven by a **dirty-row span list**
 * (`n_spans` + `spans[]`, packed `(start_row << 16) | count`, ascending + disjoint): it fast-forwards
 * the gate over clean rows (`dummy_advance`, GEN idle — nothing latches, the row keeps its memory),
 * writes only the spanned rows, and **stops early** after the last one (no forced scan to 320; the
 * gates below stay parked and retain). A full frame is the degenerate `n_spans=1, spans[0]=(0,320)`.
 * The ping-pong index toggles per **dirty** row consumed, so the buffers carry consecutive dirty
 * rows regardless of which absolute rows they are.
 *
 * The round-trip, per frame:
 *   M33   resets both buf[i] ready/consumed, publishes the span list, pre-packs the first two DIRTY
 *         rows → buf[0]/buf[1] from the framebuffer (each a ROW buffer: MSB sub-line [0..124) then
 *         LSB sub-line [124..248)), bumps each buf[i].ready, then writes cmd = CMD_RUN_FRAME + bumps
 *         m33_seq (seq last), dsb.
 *   FLPR  polls m33_seq; on a change, for CMD_RUN_FRAME runs the span-masked scan:
 *         INTB high → GSP → lead dummies → per span {fast-forward to start, then drain+write each row
 *         under the **ping-pong handshake** (wait ready != consumed, scan the gate row, set consumed
 *         = ready)} → trail dummies → INTB low. Meanwhile the M33 packs the remaining dirty rows into
 *         whichever buffer the FLPR just freed. The FLPR then bumps frame_count, writes status =
 *         dirty rows scanned + flpr_seq = m33_seq (seq last), pokes EGU20 to fire the M33's EGU20 IRQ
 *         #201.
 *   M33   wakes on the EGU20 IRQ, checks status == sum(span counts) && flpr_seq == m33_seq.
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

/* Gate + frame pin masks on P1 (bit position = P1 pin index). All µs-scale, so P1 is fine — P2 is
 * reserved for the fast bus.
 *
 * ⚠️ **DK gate/BSP pin map — moved for the app integration (issue #165).** The bring-up bench map
 * (GSP P1.11, GCK P1.12, GEN P1.04, INTB P1.06, BSP P1.07) reused the SD-SPI + VCOM-UART pins,
 * which was "safe this epic only — no VCOM and no SD bus run during bring-up". The *real app* needs
 * the SD bus (P1.06/07/11/12) to load the map + the VCOM (P1.04/05) for sensors, so the five gate/
 * BSP lines relocate to free P1 pins (they are µs-scale, so any GPIO works). **These masks MUST stay
 * in lock-step with the M33 `Output::new` pins in BOTH `main.rs` (the app) and the bring-up bin** —
 * if a pin is not broken out on your DK, remap it here *and* there. */
#define BSP_MASK  (1u << 14)   /* P1.14 = BSP  (sub-line start pulse) */
#define GSP_MASK  (1u << 0)    /* P1.00 = GSP  (gate start pulse, once per frame) */
#define GCK_MASK  (1u << 1)    /* P1.01 = GCK  (gate clock — HIGH = MSB/2-3 phase, LOW = LSB/1-3 phase) */
#define GEN_MASK  (1u << 12)   /* P1.12 = GEN  (gate output enable — latches the GCK-level-selected block) */
#define INTB_MASK (1u << 10)   /* P1.10 = INTB (frame envelope — HIGH for the whole frame write; LED1) */

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

/* Dirty-row span list cap (issue #163). Each frame the M33 publishes 1..=MAX_DIRTY_SPANS ascending
 * `(start_row, count)` spans; the masked scan fast-forwards the gate over the gaps and writes only
 * the spanned rows. 16 disjoint regions is far more than any UI produces (a full frame is ONE span
 * `(0,320)`; the bulge is one; the future renderer dirty-region block coalesces bands into a few).
 * **Must equal `MAX_DIRTY_SPANS` in the Rust `Control` (ls021_flpr.rs).** */
#define MAX_DIRTY_SPANS 16u

typedef struct {
    volatile uint32_t magic;       /* 0x00 M33: layout/version tag, checked before acting */
    volatile uint32_t m33_seq;     /* 0x04 M33: command sequence counter (the doorbell) */
    volatile uint32_t cmd;         /* 0x08 M33: command word (F3: a CMD_* code) */
    volatile uint32_t flpr_seq;    /* 0x0C FLPR: echoes the m33_seq it serviced (round-trip proof) */
    volatile uint32_t status;      /* 0x10 FLPR: ack/result (#163: dirty rows scanned; boot: FLPR_ALIVE) */
    volatile uint32_t frame_count; /* 0x14 FLPR: frames drained (bumped per CMD_RUN_FRAME) */
    volatile buf_desc_t buf[2];    /* 0x18, 0x28 row-buffer descriptors — ping-pong, toggled per DIRTY row (#163) */
    volatile uint32_t n_spans;     /* 0x38 M33: #dirty-row spans in `spans` (1 = a full frame `(0,320)`) */
    volatile uint32_t spans[MAX_DIRTY_SPANS]; /* 0x3C M33: packed `(start_row << 16) | count`, ascending, disjoint */
} flpr_control_t;                  /* 0x7C = 124 bytes */

/* Lock the cross-language contract: the Rust `Control` in ls021_flpr.rs asserts the same 124 bytes
 * (the two structs alias the same shared-RAM bytes, so the size must match exactly). 124 (0x7C) also
 * stays **below the ping-pong buffer base** (control_base + 0x100, see WRITE_BUF_ADDR), so growing it
 * with the span list never moves the buffers. */
_Static_assert(sizeof(flpr_control_t) == 124, "control block must be 124 bytes (matches the M33 Control; stays below 0x100)");

#define CTRL ((volatile flpr_control_t *)0x2003F000u)

#define LAYOUT_MAGIC 0xF1C00001u /* "F1 control block" — must match the M33 */
#define FLPR_ALIVE   0x0000A11Eu /* boot confirmation */
#define FLPR_BADMAG  0x0BADCAFEu /* booted but the control-block magic mismatched */

/* Command codes (M33 → FLPR via `cmd`). One piece of work: run a frame driven by the dirty-row span
 * list (issue #163). A full frame is the degenerate case `n_spans=1, spans[0]=(0,320)`, so this one
 * command **subsumes** the old whole-frame scan (not a parallel path); the F2 CMD_SHIFT_SUBLINE=1 was
 * already subsumed, the F3 single-buffer reuse generalised. */
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
 * ## DDR drive + speed (issue #155) — the panel latches BOTH BCK edges
 *
 * **Cracked on glass:** the panel clocks the source register on **both** `BCK` edges. The original
 * single-edge drive held each pair constant across a whole `BCK` period, so the panel captured it
 * twice → every pair landed in 4 columns (left half stretched 2×, right half dropped, 64→32 colours)
 * — invisible on solids, which is why F2–F4 + the M33 bring-up missed it. `drive_subline` now drives
 * **DDR** (a distinct pair on each edge: word `2k` before the rising edge, word `2k+1` before the
 * falling edge), so the 240-wide line ships in **60 BCK cycles** and reassembles correctly — and the
 * source shift runs ~2× faster as a bonus, which is exactly why the spec **~53 ms / 64-colour** frame
 * is reachable (the datasheet's 120-BCK/line already assumes this dual-edge throughput).
 *
 * The clock levers:
 *   - `BCK_HALF_ITERS` / `DATA_SETUP_ITERS` set each `BCK` half-period. **With DDR a data transition
 *     now sits on BOTH edges**, so each half must satisfy the panel spec independently: `BCK` width
 *     ≥660 ns (high AND low, `thwBCK`/`tlwBCK`) and data set-up ≥335 ns before *each* edge (`tsRGB`).
 *     `BCK_HALF_ITERS = 8` ≈ 660 ns half (in-spec); `DATA_SETUP_ITERS = 4` ≈ 335 ns. **Re-verify on
 *     the LA after the DDR change** — the falling-edge data timing is new. (Lower values run over-spec
 *     but worked on the bench unit; characterise before trusting across units.)
 *   - The gate timings (`GCK_SETTLE`/`GEN_SETUP`/`GEN_HIGH`) are panel **electrical minimums**
 *     (GCK↔GEN setup/hold ≥16.37 µs, GEN valid ≥24.56 µs). **Do NOT lower below their µs values** —
 *     they are correctness, not slack. (`gen_pulse` drops its *leading* setup busy — the long data
 *     shift before it already supplies the GCK↔GEN setup — and keeps the GEN-high + trailing hold.)
 *
 * The FLPR runs at the **full PLL clock** (the M33 boots it with `ClockSpeed::CK128`; there is no
 * separate VPR clock divider on this part — the early F2 "~64 MHz" was a busy-loop estimate, not a
 * lever). With DDR the per-frame floor is the source (320 rows × 2 sub-lines × 60 BCK ÷ 0.758 MHz ≈
 * **51 ms**) overlapping the gate-minimum total (~37 ms) — i.e. the spec frame. `push_frame` logs the
 * measured frame time each push — tune against that. ── */
#define ITERS_PER_US      13u /* bench calibration: busy(120) ≈ 9.4 µs on the unconfigured FLPR */
#define BCK_HALF_ITERS    2u                    /* each BCK half-period — ⚠️ OVER-SPEC bench value (~180 ns vs the ≥660 ns thwBCK/tlwBCK min; works on this unit). DDR puts data on BOTH edges → set 8 (~660 ns) for in-spec; LA-verify */
#define DATA_SETUP_ITERS  3u                    /* source data stable before EACH BCK edge (~280 ns — under the spec ~335 ns min; set 4 for in-spec, LA-verify) */
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

/* Drain one source sub-line from `base` (a byte address into the row buffer): pulse BSP, then clock
 * the `len` data words out over P2.00..05.
 *
 * **DDR drive (F5 column-doubling fix, issue #155) — verified on glass.** The panel latches the
 * source bus on **BOTH BCK edges**. The original single-edge drive held each pair constant across a
 * whole BCK period, so the panel captured it twice → every pair landed in FOUR columns (left half
 * stretched 2×, right half dropped, 64→32 colours) — invisible on solids, so F2–F4 + the M33 bring-up
 * all missed it. Driving **DDR** — word `2k` set up before the **rising** edge, word `2k+1` before the
 * **falling** edge, one distinct pair per edge, `len/2` BCK cycles for `len` words — feeds all 120
 * pairs into the full 240 columns AND clocks the sub-line out ~2× faster (the spec ~53 ms frame
 * already assumes this dual-edge throughput).
 *
 * `len` is even (124). BSP high envelopes the first rising edge (released on it). The caller has set
 * GCK to this plane's level — this touches only the source bus, never GCK/GEN. */
static void drive_subline(uint32_t base, uint32_t len)
{
    const volatile uint32_t *buf = (const volatile uint32_t *)(uintptr_t)base;

    GPIO1_OUTSET = BSP_MASK; /* BSP high — start of the sub-line (the chart's BSP envelope) */
    for (uint32_t k = 0; k < len; k += 2) {
        /* ── rising-edge column: word k ── */
        uint32_t d0 = buf[k] & DATA_MASK;
        GPIO2_OUTCLR = (~d0) & DATA_MASK;
        GPIO2_OUTSET = d0;
        busy(DATA_SETUP_ITERS);  /* data stable before the BCK rising edge */
        GPIO2_OUTSET = BCK_MASK; /* rising edge latches word k */
        if (k == 0) {
            GPIO1_OUTCLR = BSP_MASK; /* BCK(1) rose within BSP high — now release BSP */
        }
        busy(BCK_HALF_ITERS);

        /* ── falling-edge column: word k+1 ── */
        uint32_t d1 = buf[k + 1] & DATA_MASK;
        GPIO2_OUTCLR = (~d1) & DATA_MASK;
        GPIO2_OUTSET = d1;
        busy(DATA_SETUP_ITERS);  /* data stable before the BCK falling edge */
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
 * buf[i]; the M33 toggles the ping-pong index per **dirty** row consumed (issue #163: the buffers
 * carry consecutive *dirty* rows, not absolute even/odd rows). The two per-buffer counters are the
 * doorbell:
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

/* Run one frame as a **span-masked scan** (issue #163). The datasheet frame envelope is unchanged
 * from F3 (mirroring `PanelBus::fill_solid`); what changed is which rows are *written* vs.
 * *fast-forwarded*. The gate token walks a shift register top-down — `GSP` loads gate 0, every
 * `GCK` period advances one row — so to reach row N you must clock past 0..N-1, but you can:
 *   • **fast-forward a clean row** with `dummy_advance` (a `GCK` period, `GEN` idle) — it advances
 *     the gate but latches nothing, so the row keeps its retained memory; and
 *   • **stop early** — after the last dirty row + the trailing flush we drop `INTB`, leaving the
 *     gate token parked partway. Rows below the last dirty span are never advanced → they retain.
 * So a partial frame costs one cheap `GCK` advance per row from the top down to the lowest dirty
 * row, plus a full source write per row actually changed; rows below cost nothing.
 *
 * Driven by the dirty-row descriptor (`CTRL->n_spans` + `CTRL->spans`, packed `(start<<16)|count`,
 * ascending + disjoint). `gate` is the next visible row the scan will land on; `dirty` is the
 * ping-pong index, toggled per **dirty** row consumed (the M33 publishes dirty rows ascending into
 * the alternating buffers). The envelope:
 *   1. INTB high for the WHOLE frame (every frame, init-black included — INTB low = "no write").
 *   2. GSP start pulse, lead dummy advances (GSP releases on the first's GCK edge).
 *   3. For each span: fast-forward to its start, then drain+write each of its rows.
 *   4. Trailing dummy flush, then INTB low (early stop — never a forced scan to row 320).
 * A full frame is just `n_spans=1, spans[0]=(0<<16)|320`. Returns the number of **dirty** rows
 * scanned (= sum of span counts), which the M33 cross-checks. */
static uint32_t run_frame(void)
{
    GPIO1_OUTSET = INTB_MASK; /* frame envelope HIGH for the whole write */
    busy(FRAME_SETUP_ITERS);  /* thsINTB: INTB stable before GSP */
    GPIO1_OUTSET = GSP_MASK;  /* start pulse: loads the first gate */
    busy(FRAME_SETUP_ITERS);  /* thsGSP: GSP stable before the first GCK */

    for (uint32_t i = 0; i < GATE_DUMMY_LEAD; i++) {
        dummy_advance(i == 0); /* GSP releases on the very first GCK edge */
    }

    uint32_t n_spans = CTRL->n_spans;
    uint32_t gate = 0;  /* next visible row the gate token will land on (0-based) */
    uint32_t dirty = 0; /* ping-pong index across dirty rows (NOT absolute row & 1) */
    for (uint32_t s = 0; s < n_spans; s++) {
        uint32_t span = CTRL->spans[s];
        uint32_t start = span >> 16;
        uint32_t count = span & 0xFFFFu;
        while (gate < start) {
            dummy_advance(0); /* fast-forward a clean row: advance the gate, GEN idle, latch nothing */
            gate++;
        }
        for (uint32_t k = 0; k < count; k++) {
            drain_row(dirty & 1u); /* write this dirty row; its leading GCK edge advances onto `gate` */
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
            /* Run the span-masked scan (#163): fast-forward clean rows, drain+write the dirty spans
             * under the ping-pong handshake (the per-buffer ready/consumed echo happens per dirty
             * row inside drain_row), early-stop after the last dirty row. */
            rows = run_frame();
            CTRL->frame_count++; /* frames drained (liveness + the round-trip contract) */
        }

        CTRL->status = rows;  /* dirty rows scanned (0 for an unknown cmd) — M33 cross-checks == sum of span counts */
        fence();              /* status/consumed visible before the seq guard */
        CTRL->flpr_seq = seq; /* seq last = the ack guard the M33 reads */
        fence();              /* ack visible before we ring the doorbell */

        EGU20_TRIGGER0 = 1u; /* ring the M33: EGU20.EVENTS_TRIGGERED[0] -> M33 EGU20 IRQ (#201) */

        /* (F5: the per-frame LED0 "drained a frame" blink was dropped — its busy() spun the FLPR a
         * pointless ~19 ms after every frame. LED0 stays idle; the M33's EGU20 ack is the liveness
         * proof now.) */
    }
}
