---
title: Firmware updates
description: How a firmware package is released, delivered, validated, installed, and rolled back.
copy: ai
---

# Firmware updates

The device has one application slot and a small bootloader, and the update design reserves storage
for the previous image. An update package uses the [OBCU format](src:specs/OBCU_Spec.md) and is
stored as an ordinary object on the card.

Uploading a package does not install it. Installation is a separate, explicit request. The current
[board policy](src:firmware/obc-fw-nrf54l/src/flat_store.rs) refuses that request, so field
installation is disabled: the pages below describe the contract and the bootloader, which are
built and tested, not a feature a rider can use today.

## The trust model

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 442" role="img" aria-label="Current board firmware rejects ARM. In the install contract, an accepted ARM moves Idle to Armed. Installation verifies and flashes the image, then starts Trial. Confirmation returns to Idle; an unconfirmed trial restores an available rollback image.">
  <defs><marker id="software-firmware-updates-1" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
  <text class="d-tag" x="20" y="26" text-anchor="start">Install contract — currently disabled by board policy</text>
  <rect class="d-panel d-focus" x="20" y="52" width="680" height="66" rx="8" />
  <text class="d-title" x="360" y="77" text-anchor="middle">Current board: ARM → rejected</text>
  <text class="d-sub" x="360" y="97" text-anchor="middle">An uploaded package stays staged.</text>
  <rect class="d-panel" x="20" y="164" width="190" height="72" rx="8" />
  <text class="d-title" x="115" y="189" text-anchor="middle">Idle</text>
  <text class="d-sub" x="115" y="209" text-anchor="middle">Current application</text>
  <path class="d-flow" d="M210 200 L262 200" marker-end="url(#software-firmware-updates-1)" />
  <text class="d-sub" x="236" y="154" text-anchor="middle">ARM accepted</text>
  <rect class="d-panel" x="265" y="164" width="190" height="72" rx="8" />
  <text class="d-title" x="360" y="189" text-anchor="middle">Armed</text>
  <text class="d-sub" x="360" y="209" text-anchor="middle">Verify · flash · read back</text>
  <path class="d-flow" d="M455 200 L507 200" marker-end="url(#software-firmware-updates-1)" />
  <rect class="d-panel" x="510" y="164" width="190" height="72" rx="8" />
  <text class="d-title" x="605" y="189" text-anchor="middle">Trial</text>
  <text class="d-sub" x="605" y="209" text-anchor="middle">One boot to confirm</text>
  <text class="d-sub" x="360" y="272" text-anchor="middle">Power loss during install: remain Armed and retry.</text>
  <path class="d-flow" d="M605 236 L605 306" />
<path class="d-flow" d="M605 306 H115" />
  <path class="d-flow" d="M115 306 L115 239" marker-end="url(#software-firmware-updates-1)" />
  <text class="d-sub" x="360" y="300" text-anchor="middle">Confirmed → Idle with the new image</text>
  <rect class="d-panel" x="20" y="344" width="680" height="76" rx="8" />
  <text class="d-title" x="360" y="369" text-anchor="middle">Unconfirmed trial: restore an available rollback reserve</text>
  <text class="d-sub" x="360" y="389" text-anchor="middle">Verify and restore the old image, then return to Idle.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The state transitions describe the install contract and bootloader. The current board policy rejects ARM before this flow starts.</figcaption>
</figure>

The chain is built so that every failure has one safe answer:

- The device verifies the package structure, the image CRC, the signature, and the version before
  it arms anything.
- It refuses to install while a ride is recording or when the battery is too low.
- The store reserves the rollback space before it writes the boot handoff.
- The bootloader verifies the whole staged image before it erases the application.
- A power loss during installation leaves the state armed, and the next boot repeats the whole
  install.
- The new image gets one trial boot under a watchdog. An image that does not confirm itself is
  replaced from the reserve.
- A blank or invalid boot-state page means idle, and the bootloader starts the current
  application.

If the card cannot be read before the erase, the bootloader retries for about a minute and then
starts the old image. After the erase has started there is no old image to fall back to, so it
retries until it can finish the install or the rollback.

## Package validation

The package carries a CRC for integrity and an Ed25519 signature for authenticity. The signed
message covers a context string, the version, the image length, and the image, so a package cannot
be replayed as a different version. The application verifies the signature before it arms; the
bootloader verifies the image CRC before it erases. See the
[OBCU specification](src:specs/OBCU_Spec.md).

## Release publication

An owner prepares a candidate in the release console, which fixes the source commit and the
requirement revision. Candidate verification runs the ordinary CI suite and builds the firmware. A
tag push publishes nothing by itself.

Publication is gated on verification, not on a green build alone: every active requirement needs an
approved coverage plan, and every test that plan cites has to pass for this candidate. A missing or
failing test blocks publication unless an administrator records an exception for that candidate,
with a reason, and the report says which requirements were accepted that way. The
[publication workflow](src:.github/workflows/verification-publish.yml) re-checks the frozen
evidence, tags the tested commit, and publishes the firmware that was already built. Nothing is
rebuilt at publication.

The GitHub release is the archive: the binaries, the package, the checksums, and the frozen
verification report. The workflow also copies the package and a small manifest to the update
service, which clients read to find the current version, its size, and its digest. There are two
channels, stable and prerelease, and the path of a published package never changes.

## Three ways a package arrives

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 416" role="img" aria-label="A release candidate triggers a build, signature, and inspection. Verified packages are published to GitHub and the update service. The companion or builder can upload a published or local package with PUT. Current board policy rejects the following ARM request.">
  <defs><marker id="software-firmware-updates-2" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
  <text class="d-tag" x="20" y="26" text-anchor="start">Release and delivery</text>
  <rect class="d-panel" x="20" y="56" width="200" height="72" rx="8" />
  <text class="d-title" x="120" y="81" text-anchor="middle">Candidate</text>
  <text class="d-sub" x="120" y="101" text-anchor="middle">SemVer vX.Y.Z</text>
  <path class="d-flow" d="M220 92 L258 92" marker-end="url(#software-firmware-updates-2)" />
  <rect class="d-panel" x="260" y="56" width="200" height="72" rx="8" />
  <text class="d-title" x="360" y="81" text-anchor="middle">Build and sign</text>
  <text class="d-sub" x="360" y="101" text-anchor="middle">Inspect CRC + signature</text>
  <path class="d-flow" d="M460 92 L498 92" marker-end="url(#software-firmware-updates-2)" />
  <rect class="d-panel" x="500" y="56" width="200" height="72" rx="8" />
  <text class="d-title" x="600" y="81" text-anchor="middle">Publish</text>
  <text class="d-sub" x="600" y="101" text-anchor="middle">GitHub + update service</text>
  <path class="d-flow" d="M600 128 L600 164" />
<path class="d-flow" d="M600 164 H240" />
  <path class="d-flow" d="M240 164 L240 202" marker-end="url(#software-firmware-updates-2)" />
  <rect class="d-panel" x="20" y="205" width="440" height="74" rx="8" />
  <text class="d-title" x="240" y="230" text-anchor="middle">Companion (BLE) or builder (USB)</text>
  <text class="d-sub" x="240" y="250" text-anchor="middle">Download a release, or select a local package</text>
  <rect class="d-panel" x="500" y="205" width="200" height="74" rx="8" />
  <text class="d-title" x="600" y="230" text-anchor="middle">Local package</text>
  <text class="d-sub" x="600" y="250" text-anchor="middle">UPDATE.BIN</text>
  <path class="d-flow" d="M500 242 L462 242" marker-end="url(#software-firmware-updates-2)" />
  <path class="d-flow" d="M240 279 L240 317" marker-end="url(#software-firmware-updates-2)" />
  <rect class="d-panel" x="20" y="320" width="440" height="74" rx="8" />
  <text class="d-title" x="240" y="345" text-anchor="middle">PUT → staged update object</text>
  <text class="d-sub" x="240" y="365" text-anchor="middle">Upload alone does not install firmware</text>
  <path class="d-flow" d="M460 357 L498 357" marker-end="url(#software-firmware-updates-2)" />
  <rect class="d-panel d-focus" x="500" y="320" width="200" height="74" rx="8" />
  <text class="d-title" x="600" y="345" text-anchor="middle">ARM → rejected</text>
  <text class="d-sub" x="600" y="365" text-anchor="middle">Current board policy</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Release clients can stage an update package. Field installation remains disabled by the current board policy.</figcaption>
</figure>

The companion app uploads a package over Bluetooth, the map builder uploads one over USB, and
either client can upload a local file. The card is not user-accessible, so a computer cannot copy a
package onto it directly.

Both clients check the package header, the CRCs, and the size before they upload, and check the
manifest size and digest for a published package. They do not verify the signature: the device owns
the trusted key, and a check on the client would prove nothing about the device.

A client offers only a strictly newer release, and offers nothing for a development build whose
version is not a release version. After the upload it sends the arm request, naming the staged
object and its revision. The link authorizes the request: an authenticated bond, or a physical
cable.

An implementation that enables arming must refuse it when the named object is not the staged
package, when the structure, CRC, or signature is invalid, when the version is not strictly newer,
while a ride is recording, or when the battery is too low.

## The chain, layer by layer

| Check | Performed by | Purpose |
|---|---|---|
| HTTPS | client | authenticates the update service |
| Manifest size and digest | client | detects a wrong or incomplete download |
| Arm authorization | bond or cable | authorizes the install request |
| Signature | device application | authenticates the package |
| Version monotonicity | device application | prevents downgrade and reinstall |
| Image CRC | application and bootloader | detects storage or transfer corruption |
| Trial confirmation | new application | proves the new image can start |

The signature is the load-bearing check, and it does not depend on the download server: a
compromised server cannot produce an accepted package without the signing key. The manifest digest
detects a bad download and nothing more, because the manifest and the package come from the same
service.

## RRAM layout

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 467" role="img" aria-label="The main RRAM ribbon shows a 32 KiB bootloader, 1976 KiB application and small tail regions to scale. The final 28 KiB expands into a 20 KiB sEMMC stage, 4 KiB boot-state page and 4 KiB settings page. The card separately holds update and rollback objects.">
<defs><marker id="r76arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
<text class="d-tag" x="20" y="26" text-anchor="start">Firmware storage · a proportional RRAM map and an enlarged tail</text>
<text class="d-title" x="20" y="61" text-anchor="start">Application layout · proportional regions</text>
<rect x="28" y="92" width="10.373" height="52" fill="#e3ad33" stroke="#3c6b39" stroke-width="1.2" />
<rect x="38.373" y="92" width="640.55" height="52" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="678.923" y="92" width="6.483" height="52" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<rect x="685.407" y="92" width="1.297" height="52" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="686.703" y="92" width="1.297" height="52" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-title" x="350" y="115" text-anchor="middle">Application · 1976 KiB</text>
<text class="d-sub" x="350" y="135" text-anchor="middle">Starts at 0x008000</text>
<text class="d-sub" x="28" y="174" text-anchor="start">Bootloader · 32 KiB</text>
<path d="M33 144 V154 H110 V161" fill="none" stroke="#3c6b39" stroke-width="1.5" />
<text class="d-sub" x="28" y="80" text-anchor="start">0x000000</text>
<text class="d-sub" x="688" y="80" text-anchor="end">end 0x1FD000</text>
<path d="M679 144 V183 H130 V228" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M688 144 V228 H578" fill="none" stroke="#9aa884" stroke-width="1.3" />
<text class="d-title" x="175" y="212" text-anchor="start">Final 28 KiB · enlarged, same scale within this strip</text>
<rect x="130" y="230" width="320" height="57" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="290.0" y="253" text-anchor="middle">sEMMC stage</text>
<text class="d-sub" x="290.0" y="274" text-anchor="middle">20 KiB</text>
<rect x="450" y="230" width="64" height="57" fill="#cbdadb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="482.0" y="253" text-anchor="middle">State</text>
<text class="d-sub" x="482.0" y="274" text-anchor="middle">4 KiB</text>
<rect x="514" y="230" width="64" height="57" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="546.0" y="253" text-anchor="middle">Settings</text>
<text class="d-sub" x="546.0" y="274" text-anchor="middle">4 KiB</text>
<text class="d-sub" x="130" y="310" text-anchor="start">0x1F6000</text>
<text class="d-sub" x="450" y="310" text-anchor="middle">0x1FB000</text>
<text class="d-sub" x="550" y="331" text-anchor="middle">0x1FC000</text>
<text class="d-title" x="20" y="370" text-anchor="start">Card objects</text>
<rect x="180" y="346" width="220" height="62" fill="#f1cfb4" stroke="#3c6b39" stroke-width="1.2" rx="7"/>
<text class="d-label" x="290" y="371" text-anchor="middle">Update package</text>
<text class="d-sub" x="290" y="392" text-anchor="middle">kind 7</text>
<rect x="450" y="346" width="220" height="62" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" rx="7"/>
<text class="d-label" x="560" y="371" text-anchor="middle">Rollback reserve</text>
<text class="d-sub" x="560" y="392" text-anchor="middle">kind 8</text>
<text class="d-sub" x="20" y="442" text-anchor="start">Boot state is the CRC-framed app ↔ bootloader handoff. Current board policy rejects ARM.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Both RRAM strips are proportional within their own scales. The enlarged tail makes the small regions readable; the card objects have variable sizes.</figcaption>
</figure>

The bootloader has no filesystem, no radio, no display driver, and no asynchronous executor. It
uses blocking storage and RRAM operations, because everything it must do has to work when the
application does not exist. It does keep the display's polarity waveform and the watchdog running
during an install.

## Implementation

- OBCU and boot-state formats: [`OBCU_Spec.md`](src:specs/OBCU_Spec.md)
- Install protocol: [`FLAT_Store_Protocol.md`](src:specs/FLAT_Store_Protocol.md)
- Shared update logic: [`obc-dfu`](src:firmware/obc-dfu)
- Bootloader: [`obc-boot`](src:firmware/obc-boot)
- Package tool: [`obc-mkimage`](src:host/obc-mkimage)
- Release workflow: [`release.yml`](src:.github/workflows/release.yml)
- Web release client: [`release.ts`](src:builder/app/src/lib/firmware/release.ts)
- iOS release client: [`Firmware`](src:companion-ios/Packages/OBCKit/Sources/OBCTransport/Firmware)
