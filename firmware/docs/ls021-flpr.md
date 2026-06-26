# LS021B7DD02 FLPR backend — toolchain, memory & boot spec (STATUS: F0 LANDED)

The **normative reference** for moving the Sharp **LS021B7DD02** waveform generation off
the Cortex-M33 and onto the nRF54L15's **FLPR** (the VPR RISC-V coprocessor). Epic [#149];
this doc is the deliverable of **F0 [#150]** and the spec the firmware of F1–F5 is written
against. It is the FLPR-epic analog of `ls021-bringup.md` (the M33-direct epic #139 spec),
which stays the **golden reference**: the FLPR must reproduce that analyzer-verified frame.

- **F0 [#150]** — toolchain + blob boot + this spec; the FLPR toggles a GPIO. ✅ **DONE**
  (build/boot path stood up; FLPR blinks LED0 + answers a shared-RAM handshake — see
  [Verification](#verification)).
- **F1 [#151]** — M33↔FLPR comms: shared control block + VEVIF doorbells, round-trip verified.
- **F2 [#152]** — FLPR drives one source sub-line from a write buffer (LA diff vs M33).
- **F3 [#153]** — FLPR drives a full frame (init-black + solid colour on glass).
- **F4 [#154]** — ping-pong write buffers + pack from the RGB222 framebuffer (palette + shapes).
- **F5 [#155]** — `obc_platform::Panel` backend + speed-tune toward the ~53 ms spec frame.

> **What F0 proves, and why it's first.** The whole epic rests on one untested assumption:
> that we can *build* code for the FLPR, *boot* it from the M33, and have it *drive a pin*.
> F0 isolates exactly that — no panel signal, no `PanelBus`, no COM. If any of build / boot /
> GPIO-access were going to bite, it bites here, cheaply, with two LEDs and an RTT line.

## Architecture (recap from the epic)

```
   M33 (Cortex-M33)                         FLPR (RV32EMC coprocessor)
   ────────────────                         ──────────────────────────
   build.rs cross-compiles the C blob ─▶ include_bytes! ─▶ copy to FLPR RAM
   set VPR00.INITPC + CPURUN.EN=1  ───────────────────────▶  FLPR runs from INITPC
   write MAGIC to the shared word  ◀── shared SRAM ──▶       FLPR writes ALIVE
   poll the shared word → RTT "alive"                        FLPR toggles GPIO (source bus, later)
   COM (VCOM/VB/VA) free-runs on the M33  ── independent of the FLPR (safety; epic decision)
```

- **FLPR firmware = C** (epic decision). The VPR is **RV32E + M + C**; Rust's
  `riscv32emc-unknown-none-elf` is tier-3, so the blob is a small freestanding C program built
  with a RISC-V gcc, emitted to a `.bin`, and `include_bytes!`'d into the M33 image. All
  non-trivial logic (the RGB222→wire pack) stays host-tested Rust on the M33 (F4).
- **COM stays on the M33.** The proven L1 timer task (`ls021.rs::com_task`) keeps `VCOM`/`VB`/
  `VA` free-running independent of FLPR state — if the FLPR faults the panel never takes a DC
  bias. The FLPR owns gate + source only. F0 runs neither (no panel signal yet).

## Toolchain

**Any `rv32emc`-capable GNU gcc** works — the blob is freestanding (`-nostdlib
-nostartfiles`, integer ops only), so **no libgcc / newlib / rv32e multilib is linked**; only
the compiler's code-gen for `-march=rv32emc -mabi=ilp32e` is used. The repo's choice is
Homebrew's `riscv64-elf-gcc`:

```sh
brew install riscv64-elf-gcc        # GCC 16.x, bottled; same formula family as riscv64-elf-gdb
```

`build.rs` (under the `ls021-flpr` feature only) compiles `src/flpr/` into `$OUT_DIR/flpr.bin`.
The exact invocation it runs:

```sh
riscv64-elf-gcc -march=rv32emc -mabi=ilp32e \
    -Os -ffreestanding -nostdlib -nostartfiles -fno-pic \
    -ffunction-sections -fdata-sections -Wall -Wextra -Wl,--gc-sections \
    -T src/flpr/flpr.ld src/flpr/start.S src/flpr/flpr_blink.c -o flpr.elf
riscv64-elf-objcopy -O binary flpr.elf flpr.bin        # raw image, no ELF headers
```

`build.rs` probes `RISCV_GCC` (override) → `riscv64-elf-gcc` → `riscv-none-elf-gcc` →
`riscv64-unknown-elf-gcc`; the objcopy is the gcc's `-gcc`→`-objcopy` sibling (or
`RISCV_OBJCOPY`). If none is found it fails with an install hint. The carve-out + blob build
are **feature-gated**, so a plain `cargo build` (the `main.rs` map firmware) needs **no
RISC-V toolchain** and keeps the full 256 KB.

> **Why this triple, not the xPack `riscv-none-elf-gcc` the epic named.** Functionally
> identical GNU toolchains; `riscv64-elf-gcc` is a one-command Homebrew install on macOS (and
> `gcc-riscv64-unknown-elf` / equivalent on Linux CI). The `RISCV_GCC` override lets the xPack
> or Zephyr-SDK toolchain drop in unchanged.

## FLPR memory map (nRF54L15, 256 KB SRAM @ 0x2000_0000)

The FLPR executes from on-chip SRAM at the **M33-visible address** (no remap on this part):
the M33 copies the blob to the region base and writes that base into `INITPC`. Nordic's
guidance is **≤96 KB to the FLPR**. F0 reserves a generous-but-modest top slice; the
production backend (F4/F5) will shrink it (see the coexistence note). The carve-out is emitted
by `build.rs` **only under the `ls021-flpr` feature** (Cargo's `CARGO_FEATURE_LS021_FLPR`), so
`main.rs` is untouched.

| Region | Range | Size | Owner / contents |
|---|---|---|---|
| `RAM` | `0x2000_0000 .. 0x2003_8000` | 224 KB | **M33** `.data`/`.bss`/stack (the linked `RAM`) |
| `FLPR_RAM` | `0x2003_8000 .. 0x2003_F000` | 28 KB | **FLPR** image + stack; `INITPC = 0x2003_8000`, `_stack_top = 0x2003_F000` |
| `SHARED` | `0x2003_F000 .. 0x2004_0000` | 4 KB | **cross-core** handshake (F1 grows this into the control block) |

- The map lives in **two places that must agree**: `build.rs`'s carved `memory.x` (shrinks the
  M33 `RAM` to 224 KB) and `src/flpr/flpr.ld` (places the FLPR image at `0x2003_8000`,
  `_stack_top` at `0x2003_F000`). The M33 reaches `FLPR_RAM`/`SHARED` **only by hardcoded
  address** (`memcpy` + the handshake word in `ls021_flpr_bringup.rs`), never via the linker —
  so shrinking `RAM` is the entire M33-side change.
- **The FLPR region is `rwx` to the FLPR but the M33's `RAM` stops at `0x2003_8000`**, so the
  M33 linker never allocates into it. The M33 *can still write* there (it's plain SRAM) — that
  is exactly how it loads the blob and the handshake.

> **Coexistence with the 75 KB framebuffer (F4/F5, flagged not solved).** The production map
> firmware's resident set (`App` scratch + caches + the 75 KB `FbDevice64` + stack) already
> fills ~254 KB of the 256 KB (the `main.rs` `nrf-mem` budget assert, issue #124). The FLPR
> backend's real need is small — the blob (~few KB) + two write buffers (~1 KB) + FLPR stack —
> so F4/F5 will reserve a **much smaller** `FLPR_RAM` (≈8–16 KB) and retune `nrf-mem` to free
> it. F0's 28 KB is bring-up headroom, not the final budget. This is the "pin the memory map"
> task the epic called out.

## Boot sequence (M33 launcher)

Via the **VPR00** peripheral (secure alias base `0x5004_C000`; offsets from the nRF54L15 PAC):

| Register | Address | Use |
|---|---|---|
| `CPURUN` | `0x5004_C800` | bit0 `EN` = 1 → FLPR runs after core reset |
| `INITPC` | `0x5004_C808` | initial PC at start = FLPR exec base |

The M33 (`ls021_flpr_bringup.rs::start_flpr`) does, in order:

1. `copy_nonoverlapping(blob, 0x2003_8000, blob.len())` — load the image.
2. `dsb()` — make the blob **and** the pre-written handshake magic visible to the other core
   before release.
3. `INITPC = 0x2003_8000`.
4. `CPURUN = 1` — the FLPR begins executing at `INITPC` (i.e. `_start` in `start.S`).

`_start` sets `sp = _stack_top`, zeroes `.bss` (empty for the F0 blob), and calls
`flpr_main()`. Done as **raw register writes** so it depends on neither an embassy VPR driver
nor the secure/non-secure PAC alias choice.

## Pin ownership & the shared-P2 rule

`P2` is the FLPR's **dedicated** GPIO domain (Nordic: TRACE/FLPR pins live on P2) — and it is
where the LS021 **source bus + `BCK` + COM** all sit. Two cores will touch P2 at once (M33
COM, FLPR source), so the epic's rule is **absolute**:

> **Never read-modify-write `GPIO.OUT` from either core. Use only the atomic `OUTSET`
> (`+0x04`) / `OUTCLR` (`+0x08`) set/clear registers.** Each is a single write of a pin mask;
> the two cores' set/clears on disjoint pins never corrupt each other.

- **Pin *configuration*** (direction, drive) is owned by the **M33** — it calls embassy
  `Output::new` once and keeps the handle alive. The FLPR only ever pulses `OUTSET`/`OUTCLR`.
- **F0 pins:** LED0 = **P2.09** (FLPR-driven, the source-port proof + visual check); LED1 =
  **P1.10** (M33 heartbeat). GPIO P2 secure base `0x5005_0400` (`OUT 0x00`, `OUTSET 0x04`,
  `OUTCLR 0x08`, `DIR 0x10`, `DIRSET 0x14`); LED0 mask = `1<<9`.

> **SPU / secure-GPIO fallback.** F0 drives the **secure** P2 alias (`0x5005_0400`), matching
> the all-secure `nrf54l15-app-s` build. If the FLPR boots (handshake fires) but LED0 stays
> dark, the FLPR lacks secure-GPIO access: try the **non-secure** alias (`0x4005_0400`) in
> `flpr_blink.c`, and/or grant the FLPR access to GPIO P2 via the SPU `PERIPH[n].PERM`
> ownership the way the M33 build already needs the Board-Configurator ext-mem-off / 3.3 V
> VDDM settings. Discovering which is needed is part of what F0 verifies; record the answer
> here when the board confirms it.

## M33 ↔ FLPR comms (stub — filled by F1)

F0 uses the crudest possible channel: **one shared word** at `SHARED` base `0x2003_F000`. The
M33 writes `0xDEAD_BEEF` (magic) before release; the FLPR overwrites it with `0xA11E` (alive);
the M33 polls and logs. F1 ([#151]) replaces this with a **structured control block** in the
`SHARED` page (buffer pointers, status, frame counters) plus **VEVIF doorbells** (the VPR's
inter-core event interface) for per-write-buffer signalling — no more polling. Until then this
single word is the entire protocol.

## Verification

F0's checks, none depending on a later stage:

- **Build (host/CI):**
  - `cargo build --release --bin ls021_flpr_bringup --features ls021-flpr` — the blob
    compiles, `objcopy` runs, the carved `memory.x` links, and `flpr.bin` (~114 B for F0)
    embeds.
  - `cargo build --release` (default `main.rs`) — the feature gate left the 256 KB budget
    assert intact and needs **no** RISC-V toolchain (regression guard).
- **On glass (RTT + eye):** `cargo run … --features ls021-flpr` →
  - RTT logs **`FLPR alive`** after the handshake word reads `0xA11E` → the FLPR booted, ran
    code, and reached shared SRAM.
  - **LED0 (P2.09) blinks** → the FLPR has GPIO access to P2 (de-risks F2).
  - **LED1 (P1.10) also blinks** (M33 heartbeat) → both cores run concurrently.
  - If `FLPR alive` prints but LED0 is dark → the SPU/secure-GPIO finding above.
- **Logic analyzer (the golden-diff tool from epic #139):** capture **P2.09** — it toggles at
  the blob's rate **and only after `CPURUN` is set** (don't start the FLPR / hold it in reset →
  the line stays at the M33's idle low). Later stages diff FLPR-driven panel signals against
  the M33 `PanelBus` golden capture; F0 just confirms the pin moves under FLPR control.

## Build & flash

```sh
# From firmware/obc-fw-nrf54l/ (standalone crate, thumbv8m.main-none-eabihf). Needs a RISC-V
# gcc for the FLPR blob: brew install riscv64-elf-gcc
cargo run --release --bin ls021_flpr_bringup --features ls021-flpr
```

[#149]: https://github.com/timohueser/OpenBikeComputer/issues/149
[#150]: https://github.com/timohueser/OpenBikeComputer/issues/150
[#151]: https://github.com/timohueser/OpenBikeComputer/issues/151
