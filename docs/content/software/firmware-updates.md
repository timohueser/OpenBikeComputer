---
title: Firmware updates
description: The firmware release, delivery, validation, installation, trial, and rollback process.
copy: ai
---

# Firmware updates

OpenBikeComputer uses one application slot and a 32 KB bootloader. The update design can reserve storage for the previous image.

An update package uses the [OBCU format](src:specs/OBCU_Spec.md). The flat store keeps it as object kind `7`.

Uploading a package does not install it. A separate [`ARM`](src:specs/FLAT_Store_Protocol.md) request starts the install process.

`ARM` is the normative install contract. The board policy rejects it, so field installation is disabled.

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
<svg viewBox="0 0 720 430" role="img" aria-label="The firmware-update state machine has Idle, Armed, and Trial states. ARM validates the package, allocates a rollback reserve, and reboots. The bootloader verifies the staged image, flashes the application slot, and reads it back. A successful install starts one trial boot. The app confirms a healthy trial. Without confirmation, the next boot restores an available rollback reserve. A power loss during installation leaves the Armed state and repeats the install.">
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
  <text class="d-sub" x="518" y="119" text-anchor="middle">+ rollback reserve</text>

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
  <text class="d-sub" x="36" y="128" style="fill:#3c6b39">ARM — validate package,</text>
  <text class="d-sub" x="36" y="143" style="fill:#3c6b39">allocate rollback reserve, reboot</text>

  <!-- Armed -> Bootloader -->
  <line x1="518" y1="126" x2="518" y2="182" class="d-flow" marker-end="url(#fu-a)" />
  <text class="d-sub" x="508" y="158" text-anchor="end">reboot into obc-boot</text>
  <!-- idempotent self loop -->
  <path d="M596 96 C 664 96, 664 168, 600 172" fill="none" stroke="#9aa884" stroke-width="1.3" stroke-dasharray="4 4" marker-end="url(#fu-g)" />
  <text class="d-sub" x="560" y="44" text-anchor="middle" style="fill:#6b7758;font-size:9px">power loss mid-install ⇒ still Armed — redo</text>

  <!-- Bootloader -> Trial -->
  <line x1="518" y1="254" x2="518" y2="310" class="d-flow" marker-end="url(#fu-a)" />
  <text class="d-sub" x="600" y="286" text-anchor="middle" style="fill:#3c6b39">flash ok → Trial</text>

  <!-- Bootloader -> Idle (bad stage) -->
  <line x1="426" y1="218" x2="192" y2="218" class="d-hot" marker-end="url(#fu-c)" />
  <text class="d-sub" x="308" y="210" text-anchor="middle" style="fill:#a9501c">verify fails — arm cleared,</text>
  <text class="d-sub" x="308" y="234" text-anchor="middle" style="fill:#a9501c">old app intact (zero cost)</text>

  <!-- Trial -> Idle (confirm, green) -->
  <path d="M438 336 C 300 340, 220 300, 176 258" fill="none" stroke="#3c6b39" stroke-width="2" marker-end="url(#fu-a)" />
  <text class="d-sub" x="310" y="354" text-anchor="middle" style="fill:#3c6b39">app confirms healthy → Idle</text>

  <!-- Trial -> Idle (rollback, coral) -->
  <path d="M446 366 C 280 400, 150 340, 116 258" fill="none" class="d-hot" marker-end="url(#fu-c)" />
  <text class="d-sub" x="250" y="392" text-anchor="middle" style="fill:#a9501c">no confirm next boot → restore available reserve</text>
</svg>
<figcaption><code>ARM</code> validates the package and reserves rollback storage. The bootloader verifies the image before erase. The app confirms a healthy trial. Without confirmation, the bootloader restores the old image when a reserve exists.</figcaption>
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

Pushing a SemVer `v*` tag starts [`release.yml`](src:.github/workflows/release.yml). The tag version must match the board-crate version.

A published release requires a public key that differs from the committed test key. It also requires the release signing seed.
A manual dry run can use the test key, but it publishes nothing.

The workflow builds the bootloader and application. It converts the application to binary, wraps it in OBCU, and signs it.

`obc-mkimage inspect` checks both CRC values and the signature before publication.

### Release archive and download service

The GitHub release is the versioned archive. It contains release notes, ELF files, the OBCU package, and checksums.

The workflow copies the package and manifest to `updates.openbikecomputer.com`. This service permits browser downloads with CORS.

### Manifest

Clients read this JSON file:

```json
{
  "version": "v1.3.0",
  "bytes": 1204208,
  "sha256": "…64 lowercase hex…",
  "url": "https://updates.openbikecomputer.com/fw/v1.3.0/UPDATE.BIN",
  "notes": "https://github.com/…/releases/tag/v1.3.0"
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
<svg viewBox="0 0 720 512" role="img" aria-label="A release tag starts the release workflow. The workflow builds, signs, and inspects the OBCU package. GitHub stores the release archive. The update service supplies stable and prerelease manifests. The companion app and map builder upload the package. A user can also select a local package in either client. The device stores the package as an object. A separate ARM request validates and installs it.">
  <defs>
    <marker id="ota-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="ota-g" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#9aa884" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">Release, delivery, and installation</text>

  <!-- the tag -->
  <rect class="d-panel" x="24" y="42" width="118" height="58" rx="10" />
  <text class="d-title" x="83" y="70" text-anchor="middle">vX.Y.Z</text>
  <text class="d-sub" x="83" y="88" text-anchor="middle">a pushed git tag</text>
  <line x1="144" y1="71" x2="180" y2="71" class="d-flow" marker-end="url(#ota-a)" />

  <!-- the workflow -->
  <rect class="d-panel-2" x="184" y="36" width="254" height="72" rx="12" style="fill:#f4ecd6" />
  <text class="d-label" x="311" y="56" text-anchor="middle" style="fill:#a9501c">release.yml</text>
  <text class="d-sub" x="311" y="74" text-anchor="middle">build → objcopy → wrap + SIGN</text>
  <text class="d-sub" x="311" y="89" text-anchor="middle">inspect CRC values and signature</text>
  <text class="d-sub" x="311" y="103" text-anchor="middle">before publication</text>
  <line x1="440" y1="71" x2="474" y2="71" class="d-flow" marker-end="url(#ota-a)" />

  <!-- the release -->
  <rect class="d-panel" x="478" y="36" width="218" height="72" rx="12" />
  <text class="d-title" x="587" y="58" text-anchor="middle">GitHub Release</text>
  <text class="d-sub" x="587" y="76" text-anchor="middle">the source of truth</text>
  <text class="d-sub" x="587" y="91" text-anchor="middle">notes · ELFs · SHA256SUMS</text>
  <text class="d-sub" x="587" y="105" text-anchor="middle" style="fill:#3c6b39">versioned archive</text>

  <!-- mirror -->
  <line x1="311" y1="108" x2="311" y2="142" class="d-flow" marker-end="url(#ota-a)" />
  <text class="d-sub" x="319" y="130">mirror</text>

  <!-- the serving edge -->
  <rect class="d-panel-2" x="112" y="144" width="496" height="112" rx="12" style="fill:#eef2df" />
  <text class="d-label" x="360" y="167" text-anchor="middle" style="fill:#3c6b39">updates.openbikecomputer.com — the serving edge</text>
  <text class="d-sub" x="128" y="192">fw/&lt;tag&gt;/UPDATE.BIN</text>
  <text class="d-sub" x="592" y="192" text-anchor="end">written for every tag · immutable</text>
  <text class="d-sub" x="128" y="212">fw/manifest.json</text>
  <text class="d-sub" x="592" y="212" text-anchor="end">the "latest" pointer · STABLE tags only</text>
  <text class="d-sub" x="128" y="232">fw/prerelease/manifest.json</text>
  <text class="d-sub" x="592" y="232" text-anchor="end">opt-in · never moves "latest"</text>
  <text class="d-sub" x="360" y="250" text-anchor="middle" style="font-size:9px;fill:#a9501c">the update service permits browser downloads with CORS</text>

  <!-- fan out -->
  <path d="M300 256 C 240 268, 180 272, 130 284" fill="none" class="d-flow" marker-end="url(#ota-a)" />
  <line x1="360" y1="256" x2="360" y2="284" class="d-flow" marker-end="url(#ota-a)" />
  <path d="M692 108 C 716 180, 700 250, 606 282" fill="none" stroke="#9aa884" stroke-width="1.3" stroke-dasharray="4 4" marker-end="url(#ota-g)" />
  <text class="d-sub" x="648" y="188" text-anchor="middle" style="font-size:9px;fill:#6b7758">manual</text>
  <text class="d-sub" x="648" y="201" text-anchor="middle" style="font-size:9px;fill:#6b7758">package</text>
  <text class="d-sub" x="648" y="214" text-anchor="middle" style="font-size:9px;fill:#6b7758">selection</text>

  <!-- the three clients -->
  <rect class="d-panel" x="24" y="290" width="208" height="58" rx="10" />
  <text class="d-title" x="128" y="312" text-anchor="middle">companion app</text>
  <text class="d-sub" x="128" y="329" text-anchor="middle">manifest → download → BLE</text>
  <text class="d-sub" x="128" y="343" text-anchor="middle">sha256 checked on the phone</text>

  <rect class="d-panel" x="256" y="290" width="208" height="58" rx="10" />
  <text class="d-title" x="360" y="312" text-anchor="middle">map builder</text>
  <text class="d-sub" x="360" y="329" text-anchor="middle">manifest → download → USB</text>
  <text class="d-sub" x="360" y="343" text-anchor="middle">web + desktop, one parser</text>

  <rect class="d-panel" x="488" y="290" width="208" height="58" rx="10" />
  <text class="d-title" x="592" y="312" text-anchor="middle">local package</text>
  <text class="d-sub" x="592" y="329" text-anchor="middle">select UPDATE.BIN</text>
  <text class="d-sub" x="592" y="343" text-anchor="middle">in a transfer client</text>

  <path d="M128 348 C 128 360, 180 362, 212 369" fill="none" class="d-flow" marker-end="url(#ota-a)" />
  <line x1="360" y1="348" x2="360" y2="369" class="d-flow" marker-end="url(#ota-a)" />
  <path d="M592 348 C 592 360, 540 362, 508 369" fill="none" class="d-flow" marker-end="url(#ota-a)" />

  <!-- staged -->
  <rect class="d-panel-2" x="210" y="372" width="300" height="38" rx="9" style="fill:#f8efe4" />
  <text class="d-label" x="360" y="390" text-anchor="middle" style="fill:#a9501c">update-package object · kind 7</text>
  <text class="d-sub" x="360" y="404" text-anchor="middle">staged — not installed</text>

  <!-- the wall, with exactly one door -->
  <line x1="24" y1="436" x2="300" y2="436" class="d-hot" stroke-dasharray="6 5" />
  <line x1="420" y1="436" x2="696" y2="436" class="d-hot" stroke-dasharray="6 5" />
  <text class="d-sub" x="292" y="429" text-anchor="end" style="font-size:9.5px;fill:#a9501c">staging is not installing</text>
  <text class="d-sub" x="428" y="429" style="font-size:9.5px;fill:#3c6b39">one way through: ARM</text>
  <line x1="360" y1="410" x2="360" y2="452" class="d-flow" marker-end="url(#ota-a)" />

  <!-- the device -->
  <rect class="d-panel" x="250" y="454" width="220" height="46" rx="11" />
  <text class="d-title" x="360" y="476" text-anchor="middle">the device</text>
  <text class="d-sub" x="360" y="492" text-anchor="middle">validates, arms, installs</text>
</svg>
<figcaption>The release workflow signs and checks the package. Clients upload the package with <code>PUT</code>. The package remains staged until a separate <code>ARM</code> request succeeds.</figcaption>
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

The device rejects `ARM` if any of these conditions apply:

- The object ID or revision does not identify the staged package.
- The OBCU structure, CRC, or Ed25519 signature is invalid.
- The package version is not strictly newer than the running version.
- A ride is recording.
- The battery is below the install threshold.

On success, the device commits a rollback reserve and writes the boot handoff. It sends the response before reboot.

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
<svg viewBox="0 0 720 300" role="img" aria-label="The RRAM contains a 32 KB bootloader, a 1976 KB application slot, a 20 KB sEMMC stage, a 4 KB boot-state page, and a 4 KB settings page. The flat store contains an update-package object and a rollback-reserve object.">
  <text class="d-tag" x="20" y="22">RRAM partition — one app slot, a 32 KB bootloader, the blob-stage carve, two small pages</text>

  <!-- RRAM bar -->
  <rect class="d-panel-2" x="24" y="70" width="60" height="72" rx="6" style="fill:#f4ecd6" />
  <text class="d-sub" x="54" y="102" text-anchor="middle" style="fill:#a9501c">obc-boot</text>
  <text class="d-sub" x="54" y="118" text-anchor="middle">32 KB</text>

  <rect class="d-panel" x="86" y="70" width="290" height="72" rx="6" />
  <text class="d-title" x="231" y="102" text-anchor="middle">app slot</text>
  <text class="d-sub" x="231" y="120" text-anchor="middle">obc-fw-nrf54l, linked at 0x8000 · 1976 KB</text>

  <rect class="d-panel-2" x="378" y="70" width="68" height="72" rx="6" style="fill:#f8efe4" />
  <text class="d-sub" x="412" y="98" text-anchor="middle" style="fill:#a9501c">SEMMC_</text>
  <text class="d-sub" x="412" y="112" text-anchor="middle" style="fill:#a9501c">STAGE</text>
  <text class="d-sub" x="412" y="130" text-anchor="middle">20 KB</text>

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
  <text class="d-sub" x="372" y="176" style="fill:#6b7758;font-size:9.5px">0x1F6000</text>
  <text class="d-sub" x="448" y="160" style="fill:#6b7758;font-size:9.5px">0x1FB000</text>
  <text class="d-sub" x="516" y="176" style="fill:#6b7758;font-size:9.5px">0x1FC000</text>

  <text class="d-sub" x="303" y="206" text-anchor="middle" style="fill:#3c6b39">the BOOT_STATE page is the only app ↔ bootloader control channel — a CRC-framed blob, torn ⇒ Idle</text>

  <!-- SD card -->
  <rect class="d-panel" x="600" y="70" width="104" height="150" rx="10" />
  <text class="d-label" x="652" y="94" text-anchor="middle">flat store</text>
  <rect class="d-panel-2" x="616" y="108" width="72" height="42" rx="7" style="fill:#f8efe4" />
  <text class="d-sub" x="652" y="126" text-anchor="middle" style="fill:#a9501c">UPDATE</text>
  <text class="d-sub" x="652" y="140" text-anchor="middle" style="fill:#a9501c">kind 7</text>
  <rect class="d-panel-2" x="616" y="160" width="72" height="42" rx="7" />
  <text class="d-sub" x="652" y="178" text-anchor="middle">ROLLBACK</text>
  <text class="d-sub" x="652" y="192" text-anchor="middle">kind 8</text>
</svg>
<figcaption>The app writes the boot handoff to <code>BOOT_STATE</code>. It copies the sEMMC image to <code>SEMMC_STAGE</code>. The bootloader reads the update and rollback objects through absolute block ranges.</figcaption>
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
