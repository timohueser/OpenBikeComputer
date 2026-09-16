---
title: Firmware updates
description: The firmware release, delivery, validation, installation, trial, and rollback process.
copy: ai
---

# Firmware updates

OpenBikeComputer uses one application slot and a 32 KB bootloader. The update design can reserve storage for the previous image.

An update package uses the [OBCU format](src:specs/OBCU_Spec.md). The flat store keeps it as object kind `7`.

Uploading a package does not install it. The [`ARM`](src:specs/FLAT_Store_Protocol.md) contract
defines a separate request to validate and start installation.

The current [board policy](src:firmware/obc-fw-nrf54l/src/flat_store.rs) rejects every `ARM` request.
Field installation is disabled. The sections below distinguish package delivery from the install
contract and bootloader behavior.

## The trust model

The `ARM` contract and boot chain have these properties:

- The device verifies the OBCU structure, image CRC, Ed25519 signature, and version before arming.
- The device refuses an update during ride recording or when battery power is insufficient.
- The flat-store service allocates a rollback reserve before it writes the boot handoff.
- The bootloader verifies the complete staged image before it erases the application slot.
- A power loss during installation leaves the state as `Armed`. The next boot repeats the complete install.
- The new image gets one trial boot. After an unconfirmed trial, the bootloader restores an available reserve.
- The bootloader starts a 24-second watchdog before the trial. A stalled trial resets into the unconfirmed path.
- A blank or invalid boot-state page means `Idle`. The bootloader starts the current application.

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

If the card is unreadable before erase, the bootloader retries for approximately one minute.
It then clears the arm and starts the old image.

After erase starts, the bootloader retries until it can complete the install or rollback.

## Package validation

The package uses CRC-32 for integrity and Ed25519 for authenticity. The signed message contains:

- the context string `"OBCUv2-sig\0"`;
- the version string;
- the image length;
- the application image.

The application verifies the signature before it writes `Armed`. The bootloader verifies the image CRC before erase.

The OBCU container format remains header version `1`. The signature marker uses reserved header bytes.
The signature follows the application image.

This layout lets the installed bootloader read current packages. See the [OBCU specification](src:specs/OBCU_Spec.md).

## Release publication

An owner prepares a candidate in the [release console](https://releases.openbikecomputer.com).
The candidate fixes a source commit and a requirement revision. Its SemVer version must match the
board-crate version. [Candidate verification](src:.github/workflows/verification-candidate.yml)
runs the complete ordinary CI suite and calls the [firmware build](src:.github/workflows/release.yml).
A tag push does not publish firmware.

A published release requires a public key that differs from the committed test key. It also requires the release signing seed.
A manual dry run can use the test key, but it publishes nothing.

The workflow builds the bootloader and application. It converts the application to binary, wraps it in OBCU, and signs it.

`obc-mkimage inspect` checks both CRC values and the signature before publication.

### Release archive and download service

Each active system requirement must have linked tests. Every linked automated test needs a pass
from this candidate's CI run. Every linked manual test needs a recorded pass for this candidate.
The owner can attach input files to manual procedures and evidence files to manual results. A new
candidate needs new manual results. Missing tests, skipped tests, and failed checks block publication
unless an administrator records a requirement exception for that candidate. An exception needs a
reason and retains the original test results. It does not carry into another candidate. The report
and release notes distinguish accepted exceptions from verified requirements.

An included requirement marked **Definition incomplete** blocks publication. Resolve its definition
in a new requirement revision and prepare a new candidate. **Implementation needed** also blocks
publication, but an administrator can accept a candidate exception for this known gap. Requirements
marked **Excluded from releases** do not need passing verification. The final review, report, and
release notes list them separately; they are not counted as verified. Exclusion applies to future
candidates until the owner removes the label. Exceptions cannot bypass incomplete definitions,
failed CI, firmware signing, missing build files, or evidence provenance checks.

When the release gate passes, the owner can publish. The [publication workflow](src:.github/workflows/verification-publish.yml)
checks the frozen evidence and retained file hashes again. It creates the tag at the tested commit
and publishes the retained firmware without a rebuild.

The GitHub release is the versioned archive. It contains ELF files, the OBCU package, checksums,
and the frozen verification report in HTML and JSON. Requirement revisions and evidence live in
the console database. Git contains the application and workflow code.

The workflow copies the package and manifest to `updates.openbikecomputer.com`. This service permits browser downloads with CORS.

### Manifest

Clients read this JSON file:

```json
{
  "version": "v1.3",
  "bytes": 1204208,
  "sha256": "…64 lowercase hex…",
  "url": "https://updates.openbikecomputer.com/fw/v1.3.0/UPDATE.BIN",
  "notes": "https://github.com/…/releases/tag/v1.3"
}
```

Clients validate all required fields. The URL must use HTTPS. The byte count must be positive.
The digest must contain 64 hexadecimal characters.

HTTP 404 means that the channel has no published release. Clients ignore unknown fields.

### Release channels

The service uses three object names:

| Object | Written by | Role |
|---|---|---|
| `fw/<tag>/UPDATE.BIN` | every tag | immutable package |
| `fw/manifest.json` | stable tags only | default channel pointer |
| `fw/prerelease/manifest.json` | SemVer prerelease tags only | opt-in channel pointer |

A prerelease tag updates only the prerelease manifest. A stable tag updates only the stable manifest.

The package path for each tag is immutable.

### Version comparison

The device reports its firmware version through BLE Device Information or the USB EP0 request.

The TypeScript and Swift clients use the same SemVer rules. They ignore build metadata and do not offer a downgrade.

A development build reports a Git hash. Clients do not offer automatic updates when the running version is not SemVer.

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

## Three ways an update arrives

A package can arrive in three ways:

- The companion app downloads a published package and uploads it through BLE.
- The map builder downloads a published package and uploads it through USB.
- A user selects a local `UPDATE.BIN` package in either client. The client uploads it through BLE or USB.

The card is not user-accessible. A computer cannot copy a package directly to the card.

Both clients validate the OBCU header, header CRC, image CRC, signature marker, and size before upload.
They do not verify the signature. The device owns the trusted public key.

For published packages, clients also check the manifest byte count and SHA-256 digest.
They obtain the running version from BLE Device Information or the USB EP0 request.
They offer only a strictly newer SemVer release. They do not offer automatic updates for a development version.

Each client uploads the package with `PUT` as object kind `7`. This operation only stages the package.
The client then sends `ARM` with the package object ID and expected revision.
BLE authenticates the control channel. USB enumeration authorizes the request. The device requires no on-device confirmation.

An implementation that enables `ARM` must reject it if any of these conditions apply:

- The object ID or revision does not identify the staged package.
- The OBCU structure, CRC, or Ed25519 signature is invalid.
- The package version is not strictly newer than the running version.
- A ride is recording.
- The battery is below the install threshold.

The install contract requires a rollback reserve and boot handoff before success.
It requires the response before reboot. The current board does not enter this path.

The app records the package version and arm generation before reboot. The bootloader records the install result.
After boot, the app uses both records to show one result message. A normal boot shows no update message.

## The chain, layer by layer

Each check has one purpose:

| Check | Performed by | Purpose |
|---|---|---|
| HTTPS | client | authenticates the update service |
| Manifest size and SHA-256 | client | detects a wrong or incomplete download |
| `ARM` authorization | BLE authentication or USB enumeration | authorizes the install request |
| Ed25519 signature | device application | authenticates the package |
| Version monotonicity | device application | prevents downgrade and reinstall |
| Image CRC-32 | application and bootloader | detects storage or transfer corruption |
| Trial confirmation | new application | proves that the new image can start |

The signature does not depend on the download server.
A compromised server cannot create an accepted package without the signing key.

The SHA-256 digest does not authenticate the package. The manifest and package come from the same service.

The bootloader verifies the complete image CRC before erase. It restores an available rollback reserve after an unconfirmed trial.

## RRAM layout

The bootloader and application use one fixed RRAM layout. The application starts at `0x8000`.

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

The bootloader has no filesystem, BLE stack, display driver, or asynchronous executor. It uses blocking storage and RRAM operations.

During installation, the bootloader keeps the display COM waveform active.
It also keeps the watchdog active when the current state requires it.

## Implementation

- OBCU and boot-state formats: [`OBCU_Spec.md`](src:specs/OBCU_Spec.md)
- Install protocol: [`FLAT_Store_Protocol.md`](src:specs/FLAT_Store_Protocol.md)
- Flat-store layout: [`FLAT_Store_Format.md`](src:specs/FLAT_Store_Format.md)
- Shared DFU logic: [`obc-dfu`](src:firmware/obc-dfu)
- Bootloader: [`obc-boot`](src:firmware/obc-boot)
- Package tool: [`obc-mkimage`](src:host/obc-mkimage)
- Release workflow: [`release.yml`](src:.github/workflows/release.yml)
- Web release client: [`release.ts`](src:builder/app/src/lib/firmware/release.ts)
- iOS release client: [`Firmware`](src:companion-ios/Packages/OBCKit/Sources/OBCTransport/Firmware)
