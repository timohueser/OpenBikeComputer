/* flpr_comms.c — LS021 FLPR F1 comms blob (issue #151, epic #149).
 *
 * Successor to the F0 blink blob (#150): promotes the crude one-word "alive" handshake into the
 * real **bidirectional control channel** the ping-pong write-buffer handoff (F4) needs — a
 * structured shared-RAM control block + a doorbell each way, round-trip verified. Still no panel
 * signal: this is the comms analog of the M33 bring-up's L1, a small isolated step. The M33
 * launcher is `src/bin/ls021_flpr_bringup.rs`; the spec (control-block layout, channel map,
 * memory-ordering rule, and the VEVIF-vs-EGU story) is `firmware/docs/ls021-flpr.md`.
 *
 * The round-trip, per command N:
 *   M33   writes cmd=N + bumps m33_seq (seq last), dsb.
 *   FLPR  polls m33_seq in shared RAM; on a change reads cmd, writes status = N ^ 0xA11E and
 *         flpr_seq = m33_seq (seq last), then pokes EGU20.TASKS_TRIGGER[0] to fire the M33's
 *         EGU20 IRQ; finally pulses LED0 (P2.09) N times as a by-eye / logic-analyzer marker.
 *   M33   wakes on the EGU20 IRQ, reads back status/flpr_seq, checks the round-trip.
 *
 * **Why shared RAM + EGU, not VEVIF (the bring-up lesson).** The epic named the VPR's VEVIF
 * mailboxes, but on this bare-metal setup both VEVIF directions are walled: a VEVIF *task* the M33
 * rings never latched into the FLPR's TASKS CSR (even after unlocking RT-peripheral CSR access and
 * enabling INTEN), and a VEVIF *event* the FLPR raises does reach the app's EVENTS_TRIGGERED but
 * can't be gated to the M33 NVIC — the app-side VPR00 INTEN refuses writes (reads back 0) without
 * SoC-level init we don't replicate. So M33->FLPR rides the shared-RAM sequence (the FLPR is a
 * dedicated core; polling is correct and is exactly the F4 ping-pong handshake), and FLPR->M33 is
 * a real interrupt bounced off an EGU (see below). Details in firmware/docs/ls021-flpr.md.
 *
 * Freestanding (see start.S + flpr.ld): no libc/libgcc, integer ops + a fence only — no CSRs, so
 * no rv32 multilib and no Zicsr. Cross-core-safe: the LED + the EGU register are driven through
 * single writes (GPIO OUTSET/OUTCLR, EGU TASKS_TRIGGER), never a read-modify-write of a shared
 * register, so the M33 (COM, later) and the FLPR (source, later) never collide on port P2.
 */

#include <stdint.h>

/* ── GPIO P2, secure alias (nRF54L15 base 0x5005_0400). OUTSET/OUTCLR each a single atomic
 * write, no read-modify-write of OUT. LED0 = P2.09 is the FLPR's by-eye / LA response line. ── */
#define GPIO2_OUTSET (*(volatile uint32_t *)0x50050404u)
#define GPIO2_OUTCLR (*(volatile uint32_t *)0x50050408u)
#define LED0_MASK    (1u << 9) /* on-board LED0 = P2.09 */

/* FLPR → M33 doorbell via EGU20 (the "software interrupt" peripheral, secure alias 0x500C_9000).
 * Writing TASKS_TRIGGER[0] raises EGU20.EVENTS_TRIGGERED[0]; the M33 enables EGU20.INTEN[0] (a
 * NORMAL writable peripheral, unlike VPR00 whose app-side INTEN the M33 cannot write) and takes
 * the EGU20 IRQ. A plain peripheral write from the FLPR — same mechanism as the GPIO above. */
#define EGU20_TRIGGER0 (*(volatile uint32_t *)0x500C9000u)

/* ── Shared control block at the SHARED-page base 0x2003_F000 (see flpr.ld / memory.x). Layout
 * is normative and identical to the Rust `Control` in ls021_flpr_bringup.rs — keep them in sync
 * (firmware/docs/ls021-flpr.md). All fields u32, little-endian. ── */
typedef struct {
    uint32_t ptr;      /* write-buffer base (F4; unused in F1) */
    uint32_t len;      /* write-buffer length (F4) */
    uint32_t ready;    /* M33 set when filled (F4) */
    uint32_t consumed; /* FLPR set when drained (F4) */
} buf_desc_t;          /* 16 bytes */

typedef struct {
    volatile uint32_t magic;       /* 0x00 M33: layout/version tag, checked before acting */
    volatile uint32_t m33_seq;     /* 0x04 M33: command sequence counter (the doorbell) */
    volatile uint32_t cmd;         /* 0x08 M33: command word (F1: the value N) */
    volatile uint32_t flpr_seq;    /* 0x0C FLPR: echoes the m33_seq it serviced (round-trip proof) */
    volatile uint32_t status;      /* 0x10 FLPR: ack/result (F1: cmd ^ 0xA11E; boot: FLPR_ALIVE) */
    volatile uint32_t frame_count; /* 0x14 FLPR: frames drained (F4; unused in F1) */
    volatile buf_desc_t buf[2];    /* 0x18, 0x28 ping-pong write-buffer descriptors (F4) */
    volatile uint32_t reserved[2]; /* 0x38 pad — forward-compat headroom */
} flpr_control_t;

/* Lock the cross-language contract: the Rust `Control` in ls021_flpr_bringup.rs asserts the same. */
_Static_assert(sizeof(flpr_control_t) == 64, "control block must be 64 bytes (matches M33 Control)");

#define CTRL ((volatile flpr_control_t *)0x2003F000u)

#define LAYOUT_MAGIC 0xF1C00001u /* "F1 control block" — must match the M33 */
#define FLPR_ALIVE   0x0000A11Eu /* boot confirmation (also the ack XOR key) */
#define FLPR_BADMAG  0x0BADCAFEu /* booted but the control-block magic mismatched */

/* Full memory fence: cross-core data ordering + a compiler barrier, so a guard field is never
 * observed before the data it guards (the M33 side uses dsb for the same contract). */
static inline void fence(void)
{
    __asm__ volatile("fence" ::: "memory");
}

/* LED pulse half-period (busy-loop). Tuned long enough to see by eye / on the LA; the absolute
 * rate depends on the (unconfigured) FLPR clock and is not relied on. The pulses run *after* the
 * EGU doorbell, so they never add to the M33's round-trip latency. */
#define PULSE_DELAY 250000u
static void pulse_delay(void)
{
    for (volatile uint32_t i = 0; i < PULSE_DELAY; i++) {
    }
}

void flpr_main(void)
{
    /* Boot handshake: confirm the control block is the layout we expect, then stamp ALIVE so the
     * M33 knows we booted and reached shared SRAM (the F0 proof, now via the control block). */
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
        /* M33→FLPR doorbell: poll the shared-RAM sequence (the M33 writes cmd then m33_seq+dsb). */
        uint32_t seq = CTRL->m33_seq;
        if (seq == last_seq) {
            continue;
        }
        fence(); /* sequence seen before we read the command it guards */
        uint32_t cmd = CTRL->cmd;
        last_seq = seq;

        CTRL->status = cmd ^ FLPR_ALIVE; /* non-trivial echo (cmd=0 would read back ALIVE) */
        fence();                         /* status visible before the seq guard */
        CTRL->flpr_seq = seq;            /* seq last = the ack guard the M33 reads */
        fence();                         /* ack visible before we ring the doorbell */

        EGU20_TRIGGER0 = 1u; /* ring the M33: EGU20.EVENTS_TRIGGERED[0] -> M33 EGU20 IRQ (#201) */

        /* By-eye / LA marker, after the ack so it never delays the round-trip: blink LED0 N times. */
        for (uint32_t i = 0; i < cmd; i++) {
            GPIO2_OUTSET = LED0_MASK;
            pulse_delay();
            GPIO2_OUTCLR = LED0_MASK;
            pulse_delay();
        }
    }
}
