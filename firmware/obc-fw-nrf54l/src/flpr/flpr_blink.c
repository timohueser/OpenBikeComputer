/* flpr_blink.c — LS021 FLPR F0 blink blob (issue #150, epic #149).
 *
 * The first code to ever run on the nRF54L15's FLPR (the VPR RISC-V coprocessor) in this
 * project. F0 isolates the riskiest unknown — the dual-core build + boot path — *before*
 * any panel signal. This blob does exactly two observable things:
 *
 *   1. Overwrite the M33's magic in the shared handshake word with "alive" (0xA11E). The
 *      M33 wrote 0xDEADBEEF there before releasing the core and polls it over RTT, so this
 *      single store proves the FLPR booted, executed, and can reach shared SRAM.
 *   2. Toggle on-board LED0 (P2.09) forever — a by-eye + logic-analyzer check that the FLPR
 *      has GPIO access to **port P2**, its dedicated pin domain. That is the exact port the
 *      LS021 source bus + BCK live on, so this also de-risks F2.
 *
 * Freestanding (see start.S + flpr.ld + firmware/docs/ls021-flpr.md): no libc, no libgcc —
 * only loads/stores/adds/branches, so the build needs no rv32e multilib. Cross-core-safe:
 * the LED is driven through the GPIO OUTSET/OUTCLR set/clear registers, never an OUT
 * read-modify-write, so M33 (COM, later) and FLPR (source, later) never collide on the
 * shared P2 port — the rule epic #149 mandates.
 */

#include <stdint.h>

/* GPIO P2, secure alias (nRF54L15 base 0x5005_0400). OUTSET sets the named bits, OUTCLR
 * clears them — each a single atomic write, no read-modify-write of OUT. If the FLPR turns
 * out to lack secure-GPIO access, the non-secure alias 0x4005_0400 is the documented
 * fallback (firmware/docs/ls021-flpr.md) — discovering that is part of what F0 verifies. */
#define GPIO2_OUTSET (*(volatile uint32_t *)0x50050404u)
#define GPIO2_OUTCLR (*(volatile uint32_t *)0x50050408u)
#define LED0_MASK    (1u << 9) /* on-board LED0 = P2.09 */

/* Cross-core handshake word at the base of the SHARED region (see flpr.ld / memory.x). */
#define SHARED_HANDSHAKE (*(volatile uint32_t *)0x2003F000u)
#define FLPR_ALIVE       0x0000A11Eu

/* Half-period of the blink as a busy-loop count. `volatile i` so the empty loop survives
 * -Os. The absolute rate depends on the (unconfigured-yet) FLPR clock and is read off the
 * logic analyzer, not relied on — any value here gives a clearly visible blink. */
#define BLINK_DELAY 20000000u

static void delay(void)
{
    for (volatile uint32_t i = 0; i < BLINK_DELAY; i++) {
    }
}

void flpr_main(void)
{
    SHARED_HANDSHAKE = FLPR_ALIVE; /* tell the M33 we're alive */

    for (;;) {
        GPIO2_OUTSET = LED0_MASK; /* LED0 on  */
        delay();
        GPIO2_OUTCLR = LED0_MASK; /* LED0 off */
        delay();
    }
}
