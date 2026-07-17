---
title: Firmware updates
description: How OpenBikeComputer updates its own firmware in the field — the SD-staged DFU trust model (verify before erase, an idempotent Armed state, a single trial boot with rollback, a torn state page that decodes to a no-op), the two delivery paths (a card sideload from Settings and a BLE push from the companion, both confirmed on the glass), and the RRAM layout the small bootloader and the app share.
---

# Firmware updates

For most of this project's life the only way to change the firmware was a
debug probe over SWD — fine on a bench, useless in a tent. A bikepacking
computer needs to take an update the way it takes a route: a file on the SD
card, or a push from the phone, applied by the device itself. This page is how
that works and, more importantly, **why it is safe to do to a device you depend
on to get home.**

The whole mechanism is one **application slot** plus a tiny **bootloader**, and a
staged update file the device installs into itself. There is no A/B pair — the
shipping image is already ~870 KB and growing, and a second full slot doesn't fit
the chip's life. Instead the design leans entirely on **verifying before erasing**
and a **rollback snapshot**, so a single slot is never left in a state that can't
boot. The byte-level formats are normative in
[`OBCU_Spec.md`](src:OBCU_Spec.md) — the same tier as the
[`OBCM`](src:OBCM_Spec.md) / [`OBCR`](src:OBCR_Spec.md) format specs; here we
cover the design and the trust model.

## The trust model

Everything below hangs off four invariants. They are not aspirations — each is a
host test in the shared [`obc-dfu`](src:firmware/obc-dfu) crate, the same
`no_std` code the app's *armer* and the bootloader's *install engine* both run.

- **Verify before erase.** The bootloader never touches the app slot until a
  CRC-32 has passed over the *complete* staged image. A truncated download, a
  bit-flipped card, a half-copied file — all of them are rejected at **zero
  cost**: the running firmware is still there, untouched.
- **`Armed` is idempotent.** Arming an update is a durable record; the staged
  file on the card is the source of truth. Lose power anywhere mid-install and
  the state is still `Armed`, so the *entire* flash pass simply reruns on the
  next boot. There is no half-installed state to recover from.
- **One trial boot, then rollback.** A freshly-flashed image boots exactly once
  on trial. It only becomes permanent when the running app *confirms* it is
  healthy (first frame presented, card mounted). An image that crashes or wedges
  before confirming is rolled back to the snapshot of its predecessor on the next
  boot — and a hardware watchdog guarantees a wedged trial *becomes* a next boot:
  the bootloader starts the dog itself, with the app's own 24 s config, right
  before jumping into the trial, so the guarantee holds even on a cold power-on
  where nothing had started a watchdog yet. The same dog is minded on the way in,
  too — the arm's warm reset carries the app's running watchdog into the
  bootloader, which adopts and feeds it through the install so a slow SD card can
  never get an update reset mid-flash.
- **A torn state page is `Idle`.** The one channel between app and bootloader is a
  CRC-framed blob in a dedicated RRAM page. Anything that doesn't cleanly decode —
  a blank page, a half-written line, a caught bit-flip — means "no pending update,
  run the app", never a garbage install. Safety is the *default* outcome of
  corruption, not a case that has to be handled.

Read as a state machine, the update is a short cycle through three states, with
the bootloader doing the dangerous work in the middle and every failure edge
landing on a bootable image:

<figure class="fig">
<svg viewBox="0 0 720 430" role="img" aria-label="The firmware-update state machine as a cycle between three states. Idle, on the left, is the normal running state. An arrow labelled 'app arms — validate CRC-32, snapshot ROLLBACK.BIN, reboot' leads up to Armed, top right. From Armed a downward arrow, labelled 'reboot into obc-boot', enters the bootloader install band in the middle right: verify the staged image over its raw SD extents, then flash the app slot and read it back. On success a downward arrow labelled 'flash ok, write Trial' reaches Trial, bottom right. From Trial two arrows return to Idle: a green one, 'app confirms healthy — writes Idle', and a coral one, 'unconfirmed at next boot — reflash ROLLBACK.BIN'. A coral horizontal arrow runs from the bootloader band straight back to Idle, labelled 'verify fails — arm cleared, old app intact'. A small self-loop on the Armed-to-bootloader path is labelled 'power loss anywhere, still Armed, redo'. A footnote reads: a torn or blank state page decodes to Idle.">
  <defs>
    <marker id="fu-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="fu-c" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7.5" markerHeight="7.5" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
    <marker id="fu-g" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#9aa884" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">The update state machine — one slot, verify before erase</text>

  <!-- Idle -->
  <rect class="d-panel" x="34" y="182" width="156" height="72" rx="12" />
  <text class="d-title" x="112" y="212" text-anchor="middle">Idle</text>
  <text class="d-sub" x="112" y="232" text-anchor="middle">running the app</text>
  <text class="d-sub" x="112" y="247" text-anchor="middle" style="fill:#3c6b39">the normal state</text>

  <!-- Armed -->
  <rect class="d-panel" x="440" y="54" width="156" height="72" rx="12" />
  <text class="d-title" x="518" y="84" text-anchor="middle">Armed</text>
  <text class="d-sub" x="518" y="104" text-anchor="middle">update staged</text>
  <text class="d-sub" x="518" y="119" text-anchor="middle">+ rollback snapshot</text>

  <!-- Bootloader band -->
  <rect class="d-panel-2" x="426" y="182" width="252" height="72" rx="12" style="fill:#f4ecd6" />
  <text class="d-label" x="552" y="206" text-anchor="middle" style="fill:#a9501c">obc-boot — install engine</text>
  <text class="d-sub" x="552" y="224" text-anchor="middle">verify CRC over raw SD extents</text>
  <text class="d-sub" x="552" y="240" text-anchor="middle">→ flash app slot → readback</text>

  <!-- Trial -->
  <rect class="d-panel" x="440" y="310" width="156" height="72" rx="12" />
  <text class="d-title" x="518" y="340" text-anchor="middle">Trial</text>
  <text class="d-sub" x="518" y="360" text-anchor="middle">new image, one boot</text>
  <text class="d-sub" x="518" y="375" text-anchor="middle" style="fill:#a9501c">unconfirmed = suspect</text>

  <!-- Idle -> Armed -->
  <path d="M170 184 C 300 120, 360 96, 438 92" fill="none" class="d-flow" marker-end="url(#fu-a)" />
  <text class="d-sub" x="292" y="128" text-anchor="middle" style="fill:#3c6b39">app arms — validate CRC-32,</text>
  <text class="d-sub" x="292" y="143" text-anchor="middle" style="fill:#3c6b39">snapshot ROLLBACK.BIN, reboot</text>

  <!-- Armed -> Bootloader -->
  <line x1="518" y1="126" x2="518" y2="182" class="d-flow" marker-end="url(#fu-a)" />
  <text class="d-sub" x="612" y="158" text-anchor="middle">reboot into obc-boot</text>
  <!-- idempotent self loop -->
  <path d="M596 96 C 664 96, 664 168, 600 172" fill="none" stroke="#9aa884" stroke-width="1.3" stroke-dasharray="4 4" marker-end="url(#fu-g)" />
  <text class="d-sub" x="672" y="120" text-anchor="middle" style="fill:#6b7758;font-size:9px" transform="rotate(90 672 120)">power loss ⇒ still Armed, redo</text>

  <!-- Bootloader -> Trial -->
  <line x1="518" y1="254" x2="518" y2="310" class="d-flow" marker-end="url(#fu-a)" />
  <text class="d-sub" x="600" y="286" text-anchor="middle" style="fill:#3c6b39">flash ok → Trial</text>

  <!-- Bootloader -> Idle (bad stage) -->
  <line x1="426" y1="218" x2="192" y2="218" class="d-hot" marker-end="url(#fu-c)" />
  <text class="d-sub" x="308" y="210" text-anchor="middle" style="fill:#a9501c">verify fails — arm cleared,</text>
  <text class="d-sub" x="308" y="272" text-anchor="middle" style="fill:#a9501c">old app intact (zero cost)</text>

  <!-- Trial -> Idle (confirm, green) -->
  <path d="M438 336 C 300 340, 220 300, 176 258" fill="none" stroke="#3c6b39" stroke-width="2" marker-end="url(#fu-a)" />
  <text class="d-sub" x="300" y="330" text-anchor="middle" style="fill:#3c6b39">app confirms healthy → Idle</text>

  <!-- Trial -> Idle (rollback, coral) -->
  <path d="M446 366 C 280 400, 150 340, 116 258" fill="none" class="d-hot" marker-end="url(#fu-c)" />
  <text class="d-sub" x="250" y="392" text-anchor="middle" style="fill:#a9501c">no confirm next boot → reflash ROLLBACK.BIN</text>
</svg>
<figcaption>The app is the only actor that can <b>arm</b> (validate the staged file, snapshot the running image, reboot) and the only one that can <b>confirm</b> (write <code>Idle</code> once it is healthy). The bootloader does the one irreversible thing — flashing the slot — but only <em>after</em> a full-image CRC passes, and it always leaves a bootable image: a bad stage clears the arm and runs the old app, an unconfirmed trial reflashes the snapshot. A torn state page short-circuits the whole diagram to <code>Idle</code>.</figcaption>
</figure>

The single stretch where the app slot doesn't hold a complete, verified image is
*inside* the bootloader's flash pass — and that window is covered by invariant 2:
the state is still `Armed` throughout, so a power loss there just reruns the pass
from the staged file. Nothing the rider can do turns the device into a
paperweight; the worst case is "reinsert the card and power-cycle".

There is one refinement to that worst case. If the card is *unreadable* while an
update is armed — it died in the drawer between arming and rebooting, or the rider
swapped in a fresh maps card — the bootloader can't stream the staged file at all.
For a rollback, or once the flash pass has already started writing the slot, it
keeps retrying forever (never abandon a slot that might be half-written). But an
`Armed` arm whose flash pass hasn't begun has touched *nothing* — the old app is
still whole at its slot base — so after about a minute of triple-blinking the
bootloader **abandons** the arm: it clears the state back to `Idle`, records that
the arm was abandoned, and boots the old firmware. The rider sees a one-time
"update abandoned — card unreadable" card and can re-arm once the card is back,
instead of staring at a device that is holding perfectly good firmware hostage to a
card that never returns.

> **CRC-32, no signatures — on purpose (v1).** Integrity is a CRC-32/IEEE over the
> whole image, end to end. There is no cryptographic signature: physical access to
> the card is already root on an open device, so the meaningful gate is the human
> at the install step, not a key. The [`OBCU`](src:OBCU_Spec.md) header reserves
> bytes for a signature scheme if internet-sourced OTA ever lands.

## Two ways an update arrives

A staged update is one file, `/UPDATE.BIN`, in the card's root — an
[`OBCU`](src:OBCU_Spec.md) container (a 64-byte header with the image length,
CRC-32, and a `git describe` version string, then the raw application image).
There are two ways it gets there, and **exactly one** way it gets installed.

- **Card sideload.** Copy `UPDATE.BIN` onto the card from any computer, put the
  card back, and choose **Settings → System → Firmware → "Install update from card"**. The
  device scans and validates the file, shows what it found (the installed version
  → the staged version, plus a no-undo warning), and installs on an encoder press.
  This is the primitive contract: a file on a card, nothing more. The row is
  disabled while a ride is recording, because arming reboots the device.
- **BLE from the companion.** The [companion app](../companion-link/) can stage the
  same `UPDATE.BIN` over the link: it uploads a `fwImage` object (§7.6 of the BLE
  spec), the device writes it to the card verbatim, and then the app sends an
  `installFw` command (§4.4) to *ask* the device to install it.

The crucial rule is shared by both paths and stated plainly in the BLE spec's
security posture: **installing always confirms on the glass.** The phone can
*stage* an image — that is all a bonded, encrypted link authorises — but it can
never arm or reboot the device on its own. `installFw` merely posts a request; the
device runs its own scan and shows a **confirm card**, and the update proceeds only
on a physical encoder press by the rider, exactly like the pairing-passkey pattern.
There are no silent installs, ever. The running firmware's version the phone
displays is read from the standard [DIS](../companion-link/) Firmware Revision
characteristic, so after a confirmed update it simply reflects the new image on the
next connect.

A press that *can't* arm is never silent either. Between the confirm press and the
reboot the device shows a **"Preparing update..."** spinner while it snapshots the
rollback and writes the boot record — and if that pass can't finish it lands a plain
**error card** on the glass rather than spinning forever. It refuses cleanly when a
ride is recording, when a just-finished ride hasn't been saved yet, or when the card
is gone, and reports an arm-time failure (the file no longer validating, the rollback
snapshot or the boot record failing to write) the same way. In every one of these
cases nothing was armed, so the rider dismisses the card and the old firmware keeps
running.

The moment the guards pass, the spinner is swapped for a static **"Installing
update"** card — the last frame the app ever paints before the reboot. A
memory-in-pixel panel holds its image without being scanned, so that card stays
readable on the glass through the entire bootloader install (the bootloader never
draws — it only keeps the panel's COM lines alternating, see below). The card is
deliberately static: a spinner would freeze mid-sweep at the reset and read as a
hang, so the copy names the blinking LED as the "still working" signal and warns to
keep power on. The next thing the rider sees is the new image booting.

The reboot's outcome is never silent either. Before rebooting into the
bootloader, the armer leaves a small breadcrumb (the staged version + the arm's
generation) in the settings page. The bootloader, in turn, records *what happened*
into the `Idle` it lands on — accepted, rolled back, or rejected before the erase —
so the first boot afterwards reads a **recorded fact** rather than guessing from
version strings (which cannot tell a rollback from a reject when the running and
staged images share a version). Reconciling the breadcrumb against that record, the
app shows a **one-time verdict card**: *"Updated to vX"* once the new image's first
healthy frame confirms the trial (or when a first install was accepted after an
unconfirmed trial), or *"UPDATE FAILED"* when the armed image is not what's running —
either the arm was never consumed (a stale or missing bootloader, which the app then
clears so it can't fire by surprise later) or the stage was rejected / rolled back. A
plain boot has no breadcrumb and shows nothing.

## The RRAM layout

The design fits in the device's non-volatile RRAM as a fixed partition the small
bootloader and the big application both agree on. The app is *always* linked at
`0x8000` — there is one build shape, no bootloader-less variant — so dev flashing
is "flash `obc-boot` once, then iterate on the app exactly as before".

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="A memory map of the device's RRAM as a horizontal bar, and the SD card beside it. The RRAM bar has four segments left to right: obc-boot, the 32-kilobyte bootloader at address 0x0; a large app slot of about 1484 kilobytes starting at 0x8000; a 4-kilobyte BOOT_STATE page at 0x17B000; and a 4-kilobyte SETTINGS page at 0x17C000. Below the segments their start addresses are labelled. To the right, a smaller SD card panel lists two files in the card root: UPDATE.BIN, the staged OBCU container, and ROLLBACK.BIN, the snapshot of the running image written by the armer.">
  <text class="d-tag" x="20" y="22">RRAM partition — one app slot, a 32 KB bootloader, two small pages</text>

  <!-- RRAM bar -->
  <rect class="d-panel-2" x="24" y="70" width="60" height="72" rx="6" style="fill:#f4ecd6" />
  <text class="d-sub" x="54" y="102" text-anchor="middle" style="fill:#a9501c">obc-boot</text>
  <text class="d-sub" x="54" y="118" text-anchor="middle">32 KB</text>

  <rect class="d-panel" x="86" y="70" width="360" height="72" rx="6" />
  <text class="d-title" x="266" y="102" text-anchor="middle">app slot</text>
  <text class="d-sub" x="266" y="120" text-anchor="middle">obc-fw-nrf54l, linked at 0x8000 · ~1484 KB</text>

  <rect class="d-panel-2" x="448" y="70" width="66" height="72" rx="6" style="fill:#eef2df" />
  <text class="d-sub" x="481" y="98" text-anchor="middle" style="fill:#3c6b39">BOOT_</text>
  <text class="d-sub" x="481" y="112" text-anchor="middle" style="fill:#3c6b39">STATE</text>
  <text class="d-sub" x="481" y="130" text-anchor="middle">4 KB</text>

  <rect class="d-panel-2" x="516" y="70" width="66" height="72" rx="6" />
  <text class="d-sub" x="549" y="102" text-anchor="middle">SETTINGS</text>
  <text class="d-sub" x="549" y="120" text-anchor="middle">4 KB</text>

  <!-- addresses -->
  <text class="d-sub" x="24" y="160" style="fill:#6b7758;font-size:9.5px">0x0000</text>
  <text class="d-sub" x="86" y="160" style="fill:#6b7758;font-size:9.5px">0x8000</text>
  <text class="d-sub" x="440" y="176" style="fill:#6b7758;font-size:9.5px">0x17B000</text>
  <text class="d-sub" x="516" y="160" style="fill:#6b7758;font-size:9.5px">0x17C000</text>

  <text class="d-sub" x="303" y="206" text-anchor="middle" style="fill:#3c6b39">the BOOT_STATE page is the only app ↔ bootloader channel — a CRC-framed blob, torn ⇒ Idle</text>

  <!-- SD card -->
  <rect class="d-panel" x="600" y="70" width="104" height="150" rx="10" />
  <text class="d-label" x="652" y="94" text-anchor="middle">SD card root</text>
  <rect class="d-panel-2" x="616" y="108" width="72" height="42" rx="7" style="fill:#f8efe4" />
  <text class="d-sub" x="652" y="126" text-anchor="middle" style="fill:#a9501c">UPDATE</text>
  <text class="d-sub" x="652" y="140" text-anchor="middle" style="fill:#a9501c">.BIN</text>
  <rect class="d-panel-2" x="616" y="160" width="72" height="42" rx="7" />
  <text class="d-sub" x="652" y="178" text-anchor="middle">ROLLBACK</text>
  <text class="d-sub" x="652" y="192" text-anchor="middle">.BIN</text>
</svg>
<figcaption>The bootloader lives in its own 32 KB slot below the app and is flashed once; the app never moves it. <code>BOOT_STATE</code> is the single 4 KB handoff page — the armer writes an <code>Armed</code> record there and the bootloader reads it, both through the shared codec, and any unclean read is <code>Idle</code>. The staged <code>UPDATE.BIN</code> and the <code>ROLLBACK.BIN</code> snapshot live on the card, not in RRAM: the app resolves them to raw SD block runs so the FAT-free bootloader can read them with plain SPI block reads.</figcaption>
</figure>

The bootloader is deliberately tiny and dumb — no FAT, no BLE, no display driver,
no async executor, just blocking GPIO + SPI + RRAMC. All the logic that could be
*wrong* (the decode, the boot decision, the install sequencing) lives upstream in
`obc-dfu` and is host-tested with mock IO; the bootloader is a thin driver that
maps the engine's outcome onto an LED code. That split is what lets the safety
invariants be *tested* rather than trusted.

The one panel courtesy it performs needs no drawing at all: on every install path
it parks the display's scan pins driven-low (so nothing floats into the glass while
the app slot is rewritten) and keeps the panel's anti-DC-bias **COM wave**
alternating in software, paced off the CPU cycle counter from the same chokepoints
that pet the watchdog. That is what lets the app's pre-painted "Installing update"
frame survive on the glass for the whole flash — and it removes a real electrical
stress: memory-in-pixel cells must never sit under a DC bias, which is exactly what
a frozen COM line would apply for the multi-ten-second install.

---

## Where this lives

- The byte formats — the `UPDATE.BIN` container and the boot-state page — normative: [`OBCU_Spec.md`](src:OBCU_Spec.md)
- The shared `no_std` core: [`obc-dfu`](src:firmware/obc-dfu) — the container + state codecs ([`image.rs`](src:firmware/obc-dfu/src/image.rs) · [`state.rs`](src:firmware/obc-dfu/src/state.rs)), the bootloader's install engine ([`engine.rs`](src:firmware/obc-dfu/src/engine.rs)), and the app-side armer ([`armer.rs`](src:firmware/obc-dfu/src/armer.rs))
- The bootloader itself, its LED codes and flash-once workflow: [`obc-boot`](src:firmware/obc-boot) ([README](src:firmware/obc-boot/README.md))
- The host tool that builds and inspects `UPDATE.BIN`: [`obc-mkimage`](src:firmware/obc-mkimage) — the `objcopy → wrap` pipeline is in the [firmware README](src:firmware/README.md)
- The BLE staging path — the `fwImage` object and `installFw` command: [the companion link](../companion-link/) (contract: [`obc-ble-interface-spec.md`](src:obc-ble-interface-spec.md) §4.4, §7.6)
- Copying a card image by hand and the release recipe: the [repo README](src:README.md)
