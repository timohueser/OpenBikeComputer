/* flpr_source.c — LS021 FLPR F2 source-shift blob (issue #152, epic #149).
 *
 * Successor to the F1 comms blob (flpr_comms.c, #151): keeps the same control block + doorbells,
 * and adds **the single most timing-critical piece of the whole epic** — the FLPR clocking out
 * **one source sub-line** from a write buffer. No gate scan, no frame envelope, no glass: just the
 * inner data-shift loop (`BSP` + 124 `BCK` + the 6 data lines), driven bring-up-slow and diffed on
 * the logic analyzer against the M33 golden capture (`PanelBus::write_data_subline` /
 * `shift_subline_with` in src/ls021.rs). The M33 launcher is src/bin/ls021_flpr_bringup.rs; the
 * spec (write-buffer format, pin/port map, the BCK budget, the LA recipe) is
 * firmware/docs/ls021-flpr.md.
 *
 * The round-trip, per command:
 *   M33   fills the SHARED-page write buffer with a 124-word test sub-line, sets buf[0].ptr/len/
 *         ready, writes cmd = CMD_SHIFT_SUBLINE, bumps m33_seq (seq last), dsb.
 *   FLPR  polls m33_seq; on a change reads cmd and, for CMD_SHIFT_SUBLINE, drains buf[0]: pulse
 *         BSP, then 124 BCK presenting each word's 6 data bits on P2.00..05. It then echoes
 *         buf[0].ready into buf[0].consumed, writes status = columns driven + flpr_seq = m33_seq
 *         (seq last), pokes EGU20.TASKS_TRIGGER[0] to fire the M33's EGU20 IRQ, and toggles LED0
 *         (P2.09) once as a by-eye liveness marker.
 *   M33   wakes on the EGU20 IRQ, checks consumed == ready and status == len, captures the sub-line
 *         on the LA.
 *
 * **Two ports, on purpose (the F2 variable).** The timing-critical bus — the 6 data lines + `BCK`
 * — is all on **P2** (`P2.00..06`, the FLPR's fast trace domain), so the hot 124× loop is single-
 * port: one `OUTSET`/`OUTCLR` pair to present data, one to pulse `BCK`. `BSP`, pulsed *once* per
 * sub-line (outside the hot loop), sits on **P1.07** — so F2 doubles as the first low-stakes proof
 * that the FLPR can drive a non-P2 (P1) GPIO at all, which F3's gate scan (`GSP`/`GCK`/`GEN`/`INTB`,
 * all on P1) will fully depend on. If `BSP` stays dead on the LA while the P2 bus toggles, that is
 * the L1 "compiles + runs != routes" lesson surfacing on P1 — see firmware/docs/ls021-flpr.md.
 *
 * Cross-core-safe (epic rule): every GPIO touch is a single OUTSET/OUTCLR write of a pin mask
 * limited to *this core's* pins (P2.00..06 + LED0 on P2, BSP on P1) — never a read-modify-write of
 * OUT — so the M33's COM lines (P2.07/08/10, later) and the FLPR's source bus never corrupt each
 * other on the shared port.
 *
 * Freestanding (see start.S + flpr.ld): no libc/libgcc, integer ops + a fence only — no CSRs, so
 * no rv32 multilib and no Zicsr.
 */

#include <stdint.h>

/* ── GPIO, secure aliases (the all-secure nrf54l15-app-s build F1 already drives). Each OUTSET/
 * OUTCLR is one atomic write, no read-modify-write of OUT. Offsets: OUT +0x00, OUTSET +0x04,
 * OUTCLR +0x08 (nRF54L15 PAC). ──
 *   P2 base 0x5005_0400 — source data + BCK + LED0 (the FLPR's fast trace domain).
 *   P1 base 0x500D_8200 — BSP (one pulse per sub-line; first FLPR write to a non-P2 port). */
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
#define BSP_MASK  (1u << 7)    /* P1.07 = BSP (sub-line start pulse) */
#define LED0_MASK (1u << 9)    /* P2.09 = on-board LED0 — by-eye "serviced a sub-line" marker */

/* FLPR → M33 doorbell via EGU20 (secure 0x500C_9000) — see flpr_comms.c / ls021-flpr.md. */
#define EGU20_TRIGGER0 (*(volatile uint32_t *)0x500C9000u)

/* ── Shared control block at the SHARED-page base 0x2003_F000 (see flpr.ld / memory.x). Layout is
 * normative and identical to the Rust `Control` in ls021_flpr_bringup.rs — keep them in sync
 * (firmware/docs/ls021-flpr.md). All fields u32, little-endian. ── */
typedef struct {
    uint32_t ptr;      /* write-buffer base (F2: the SHARED-page sub-line buffer; F4: ping-pong) */
    uint32_t len;      /* write-buffer length in words = BCK per sub-line (124) */
    uint32_t ready;    /* M33 set when filled — a token the FLPR echoes into `consumed` */
    uint32_t consumed; /* FLPR set when drained (= the serviced `ready` token) */
} buf_desc_t;          /* 16 bytes */

typedef struct {
    volatile uint32_t magic;       /* 0x00 M33: layout/version tag, checked before acting */
    volatile uint32_t m33_seq;     /* 0x04 M33: command sequence counter (the doorbell) */
    volatile uint32_t cmd;         /* 0x08 M33: command word (F2: a CMD_* code) */
    volatile uint32_t flpr_seq;    /* 0x0C FLPR: echoes the m33_seq it serviced (round-trip proof) */
    volatile uint32_t status;      /* 0x10 FLPR: ack/result (F2: columns driven; boot: FLPR_ALIVE) */
    volatile uint32_t frame_count; /* 0x14 FLPR: frames drained (F4; unused in F2) */
    volatile buf_desc_t buf[2];    /* 0x18, 0x28 ping-pong write-buffer descriptors (F2 uses buf[0]) */
    volatile uint32_t reserved[2]; /* 0x38 pad — forward-compat headroom */
} flpr_control_t;

/* Lock the cross-language contract: the Rust `Control` in ls021_flpr_bringup.rs asserts the same. */
_Static_assert(sizeof(flpr_control_t) == 64, "control block must be 64 bytes (matches M33 Control)");

#define CTRL ((volatile flpr_control_t *)0x2003F000u)

#define LAYOUT_MAGIC 0xF1C00001u /* "F1 control block" — must match the M33 */
#define FLPR_ALIVE   0x0000A11Eu /* boot confirmation */
#define FLPR_BADMAG  0x0BADCAFEu /* booted but the control-block magic mismatched */

/* Command codes (M33 → FLPR via `cmd`). F2 has exactly one piece of work. */
#define CMD_SHIFT_SUBLINE 0x00000001u /* drain buf[0] as one source sub-line */

/* Full memory fence: cross-core data ordering + a compiler barrier, so a guard field is never
 * observed before the data it guards (the M33 side uses dsb for the same contract). */
static inline void fence(void)
{
    __asm__ volatile("fence" ::: "memory");
}

/* ── Bit-bang delays (busy-loops). The FLPR clock is unconfigured at this stage, so these are
 * **LA-calibrated on the bench**, exactly like the M33 path's asm::delay counts. The target is
 * *bring-up-slow*: BCK well under the 0.758 MHz spec max so the analyzer resolves every edge —
 * speed is F5's job. Tune on the capture: if BCK reads faster than ~0.7 MHz raise BCK_HALF_ITERS;
 * the M33 golden path runs BCK at ~165 kHz (3 µs half-period). DATA_SETUP_ITERS gives the source
 * data setup before BCK rises (spec ~335 ns; held generously long here). ── */
#define BCK_HALF_ITERS   120u /* each BCK phase (high, then low) */
#define DATA_SETUP_ITERS 40u  /* data stable on P2.00..05 before the BCK rising edge */

static void busy(uint32_t iters)
{
    for (volatile uint32_t i = 0; i < iters; i++) {
    }
}

/* Drain one source sub-line from the write buffer: pulse BSP, then `len` BCK, presenting each
 * word's 6 data bits on P2.00..05. Mirrors the M33 `shift_subline_with` timing relationships:
 * BSP high envelopes BCK(1) (released on the first BCK rising edge), data is set up DATA_SETUP
 * before each BCK rise. Returns the number of columns clocked (== len), which the M33 cross-checks.
 *
 * `base` is buf[0].ptr (the descriptor mechanism F4 reuses); for F2 it points into the SHARED page.
 * Each word is `& DATA_MASK`-clean already, but we mask defensively so a stray high bit can never
 * reach BCK (P2.06) or LED0 (P2.09). */
static uint32_t drive_subline(uint32_t base, uint32_t len)
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
    return len;
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
        /* M33→FLPR doorbell: poll the shared-RAM sequence (M33 writes the buffer + cmd then
         * m33_seq+dsb). The FLPR is a dedicated core, so polling is correct — and is exactly the
         * F4 ping-pong "buffer ready" handshake. */
        uint32_t seq = CTRL->m33_seq;
        if (seq == last_seq) {
            continue;
        }
        fence(); /* sequence seen before we read the command + buffer it guards */
        uint32_t cmd = CTRL->cmd;
        last_seq = seq;

        uint32_t driven = 0;
        if (cmd == CMD_SHIFT_SUBLINE) {
            /* Read the buffer location/length from the descriptor (the F4 contract), then drain it. */
            uint32_t base = CTRL->buf[0].ptr;
            uint32_t len = CTRL->buf[0].len;
            driven = drive_subline(base, len);
            CTRL->buf[0].consumed = CTRL->buf[0].ready; /* echo the ready token = "drained" */
        }

        CTRL->status = driven; /* columns clocked (0 for an unknown cmd) — the M33 cross-checks == len */
        fence();               /* status/consumed visible before the seq guard */
        CTRL->flpr_seq = seq;  /* seq last = the ack guard the M33 reads */
        fence();               /* ack visible before we ring the doorbell */

        EGU20_TRIGGER0 = 1u; /* ring the M33: EGU20.EVENTS_TRIGGERED[0] -> M33 EGU20 IRQ (#201) */

        /* By-eye liveness marker, *after* the ack and the captured waveform so it perturbs neither:
         * one LED0 toggle per serviced sub-line. */
        GPIO2_OUTSET = LED0_MASK;
        busy(200000u);
        GPIO2_OUTCLR = LED0_MASK;
    }
}
