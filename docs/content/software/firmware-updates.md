---
title: Firmware updates
description: How OpenBikeComputer updates its own firmware in the field — the SD-staged DFU trust model, how a tagged release is published and served so every client finds it, the delivery paths (card sideload, BLE, USB — all confirmed on the glass), and the RRAM layout the bootloader and the app share.
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
[`OBCU_Spec.md`](src:specs/OBCU_Spec.md) — the same tier as the
[`OBCM`](src:specs/OBCM_Spec.md) / [`OBCR`](src:specs/OBCR_Spec.md) format specs; here we
cover the design and the trust model.

## The trust model

Everything below hangs off five invariants. They are not aspirations — each is a
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
- **Nothing unsigned gets armed.** The app verifies an Ed25519 signature over the
  staged image before an install is even possible, and an *unsigned* container is
  refused just as firmly as a badly-signed one — otherwise the check would be
  trivially skippable. See [Signed images](#signed-images-and-the-one-thing-the-bootloader-deliberately-cant-do)
  below for why the bootloader is deliberately left out of this.

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
  <text class="d-sub" x="36" y="128" style="fill:#3c6b39">app arms — validate CRC-32,</text>
  <text class="d-sub" x="36" y="143" style="fill:#3c6b39">snapshot ROLLBACK.BIN, reboot</text>

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

## Signed images, and the one thing the bootloader deliberately can't do

v1 of the format shipped with a CRC-32 and nothing else, on the honest reasoning that
physical access to the card is already root on an open device. It also reserved twelve
header bytes "for a future signature-scheme marker if internet-sourced OTA ever lands".
It landed — updates are published as GitHub releases now, and a file fetched over the
internet is a different threat from a file you copied yourself. So **OBCU v2 signs the
image**, and those reserved bytes are what it spends.

The two checks answer different questions, and both stay:

- The **CRC-32** asks *did these bytes arrive intact?* It catches a truncated download
  or a bit-flipped card, and the answer the rider gets is "this file is damaged, copy
  it again".
- The **signature** asks *are these bytes ours?* It catches a forged or tampered image,
  and the answer is "this update is not signed for this device". Telling someone to
  re-copy a perfectly intact forgery would be a lie, so these are two separate error
  cards, not one.

The signature is **Ed25519** over a deliberately narrow message: a fixed context string
`"OBCUv2-sig\0"`, then the header's version string and image length, then the image
itself. Every piece is doing a job. The context string means an OBCU signature can
never be valid in some other protocol that signs raw bytes. Covering the *version
string* is what stops re-labelling — without it, a genuinely signed v1.4.0 image could
be re-announced as v9.9.9, or as something older to walk a device backwards into a
build with a known bug, and the signature would still check out. Covering the *length*
stops a lie about how much to read and flash. The signature itself sits in a trailer
after the image, in bytes v1 had already declared ignorable.

Verification happens in the **app**, before an update can be armed — and it is a hard
gate: an unsigned container is rejected outright, not merely flagged. That last part is
the whole design. If a v1-style unsigned wrapper still installed, nobody would ever
bother forging a signature; they would just leave it off.

The bootloader, notably, does **not** verify. That looks like the wrong half to leave
out until you remember what the bootloader is: 32 KB, flashed once by probe, and never
updated by DFU. Whatever key it trusted would be frozen into the device for its entire
life, unrotatable. Putting the trust root in the half that ships with every image is
what makes rotation possible at all — a device trusts exactly the key its own firmware
was built with.

Which raises the awkward constraint that shaped the whole format change: **bootloaders
already in the field have to keep installing images built years later.** The v1 header
decoder they run rejects any header-version value but `1`, so a "v2" container cannot
actually bump that field — a bumped version would be unparseable to every boot chain
already out there, and every future update would fail the same way. So it doesn't bump.
A v2 container still says version `1`, keeps every field the bootloader reads at its
original offset, puts the scheme marker in the reserved space that was set aside for
precisely this, and hides the signature past the end of the image where v1 already
ignored bytes. The old bootloader reads a v2 file and sees exactly what it saw before;
it flashes the same bytes from the same offsets. The
[format spec](src:specs/OBCU_Spec.md) states that argument field by field, and
`obc-dfu`'s tests prove it by re-implementing the old decoder from the spec text and
running the real install engine over a v2 container.

## Publishing an update: a tag, and one small file everything reads

Everything above assumes a file that is already on the card. This is the part before
that: how a pushed git tag becomes an update that every client can find — the phone, the
builder in a browser, and a bare card reader with no app behind it at all. It is worth
stating the boundary up front, because it *is* the shape of the
design — the automation covers **delivery**, end to end, and it stops at the glass.
Nothing described in this section can install anything.

Pushing a SemVer-shaped `v*` tag runs [`release.yml`](src:.github/workflows/release.yml).
The workflow rejects malformed version input before using it in a path or command, and a
real tag's numeric core must match the board crate's version. It then builds
both flashed images in their shipping shape, `objcopy`s the app to a raw binary, and
wraps it into an OBCU container stamped with the tag and signed with the release key.
Then, before anything is published, it runs `obc-mkimage inspect` over the result as a
**gate**: inspect checks both CRC-32s *and* the signature against the release public
key compiled into the firmware it has just built, so a container that firmware would
refuse to install cannot become a release. Two further guards apply to the publishing
path alone. The job refuses a real tag while the trusted key is still the committed
*test* key — whose seed is in the repository, so anyone could sign an image such a
firmware would happily install — and it refuses one when the signing secret is absent,
rather than quietly falling back to that test key. A manual dry run *may* sign with the
test key, for the single reason that makes it safe: it publishes nothing.

### Two homes for the same bytes, and why

The tagged **GitHub release** is the source of truth. It is the versioned archive, the
release notes a rider actually reads, and where both ELFs and the
`SHA256SUMS.txt` live — the things you want years later when the question is *what
exactly was `v1.3.0`?*

It is not where the apps fetch from, and the reason is mundane rather than
architectural. A release asset's stable download URL redirects to storage that sends no
`access-control-allow-origin` header at all, so a browser `fetch` cannot read the
response — which kills the check in both hosts that have a browser inside them, the web
builder and the desktop app's webview. (GitHub's JSON API *does* send CORS headers, but
it is the rate-limited surface the design set out to stay off.) So the workflow mirrors
two objects into the project's own R2 bucket — the same object storage the
[map bakery](../formats/#the-catalog-the-map-builders-source-of-truth)
publishes its catalog into, under its own prefix — and that bucket is the **serving
edge**.

Splitting the two isn't a compromise; it's the two jobs pulling apart. A source of
truth wants immutability, a history, and a page a person can read; a serving edge wants
CORS headers, no rate limit, and a shape that never changes. Asking one host to be both
is what raised the question in the first place.

### The manifest is the abstraction

What a client reads is never the release. It is one small JSON file:

```json
{
  "version": "v1.3.0",
  "bytes": 1204208,
  "sha256": "…64 lowercase hex…",
  "url": "https://updates.openbikecomputer.com/fw/v1.3.0/UPDATE.BIN",
  "notes": "https://github.com/…/releases/tag/v1.3.0"
}
```

Five fields, and the indirection in the middle one is the point. The manifest *names*
where the image is, so the serving host can move — a different bucket, a CDN in front of
it, a mirror — by changing one workflow variable, with no client release and without
touching the trust chain, because nothing in that chain depends on who served the bytes.
The signature says whose the image is and the digest says whether it arrived whole; the
host only ever says *where*. The same manifest is attached to the GitHub release as an
asset too, so the archive describes itself without depending on the bucket at all.

The parsers treat it as a contract rather than a hint. Every required field is checked
before any of it is used — a `version` that is not a release version, a `sha256` that is
not 64 hex characters, a `bytes` that is not a positive integer, a `url` that is not
`https` — and a manifest failing any of those is reported loudly rather than partly
believed, because a half-understood manifest that still offers a download is worse than
no manifest. A plain **404 is not a failure**: it means nothing is published on that
channel, which is an ordinary answer and is treated as one. Unknown fields are ignored,
so the file can grow without a flag day.

### Two channels, which is the whole rollout lever

Three object names carry all of it:

| Object | Written by | Role |
|---|---|---|
| `fw/<tag>/UPDATE.BIN` | every tag | the image itself — versioned, and never rewritten |
| `fw/manifest.json` | **stable** tags only | the "latest" pointer, the one every client polls by default |
| `fw/prerelease/manifest.json` | SemVer **pre-release** tags only | the opt-in channel |

A SemVer pre-release tag — `v1.3.0-rc.1` — publishes its image and moves the
pre-release pointer, and it **never touches** `fw/manifest.json`. The channel decision
looks only before optional `+build` metadata, so `v1.3.0+build-2` remains stable. That single
refusal is the staged rollout. A rider on the default channel cannot be shown a release
candidate at all; a client that offers the opt-in fetches both pointers and takes
whichever names the newer version; and a specific build can still be handed to a
specific device by reaching straight for the object under its tag, which is exactly what
the immutable per-tag path is for. Nothing here needed a percentage rollout or a device
list to be able to say *"this one goes to the people who asked for it"*.

### One dialect, two parsers, three hosts

A client's entire decision is a comparison of two strings: the version the manifest
names, and the version the connected device reports over
[DIS](../companion-link/) or the USB device-information frame. So the dialect has to
mean the same thing everywhere, and it is written twice — once in TypeScript for the
builder's web and desktop hosts, once in Swift for the phone — with the Swift test
matrix a port of the TypeScript one, case for case, because two parsers that drift would
disagree about whether a rider is up to date.

The dialect is deliberately small: an optional leading `v`, a three-part numeric core,
an optional `-pre` tag, and `+build` metadata that is parsed only to be discarded, so
`1.2.0+abc1234` and `1.2.0` are one version. A pre-release sorts *before* the release
sharing its triple; its dot-separated identifiers use SemVer precedence, so `rc.10`
follows `rc.2`. A device running something newer than what is published is told so and
never offered a downgrade.

And a string that is not a release version parses as **nothing at all**, which is where
the one locked rule lives: a client that cannot read the running version never offers an
update, and it says so rather than going quiet. That is not a gap in the parsers — it is
why the firmware's fallback revision string is a bare git hash rather than anything
version-shaped. A probe-flashed development build reports a hash, every comparison
refuses instead of guessing, and nobody's work-in-progress gets overwritten by a
"newer" release. The way back onto the release track is the manual path, which is always
there.

<figure class="fig">
<svg viewBox="0 0 720 512" role="img" aria-label="How a tagged release reaches a device, drawn top to bottom. At the top left, a pushed git tag v1.3.0 feeds the release.yml workflow, which builds, objcopies, wraps and signs the image and then runs inspect as a gate — the signature must verify under the key compiled into the build. From the workflow one arrow goes right to the GitHub Release, labelled the source of truth: notes, ELFs, checksums, versioned archive. A second arrow goes down, labelled mirror, into a band for updates.openbikecomputer.com, the serving edge, which lists three objects: fw slash tag slash UPDATE.BIN, immutable and written for every tag; fw slash manifest.json, the latest pointer, written by stable tags only; and fw slash prerelease slash manifest.json, the opt-in channel that never moves latest. A note in the band explains that a GitHub release asset sends no CORS header, so a browser fetch cannot read it. Below, two arrows leave the manifest for two clients — the companion app, which downloads and checks the sha256 on the phone before sending over BLE, and the map builder, one parser across web and desktop, which does the same over USB — while a third dashed arrow runs from the GitHub Release straight to any computer, the by-hand path that copies UPDATE.BIN onto the card with no manifest and no network. All three converge on a band reading UPDATE.BIN on the card: staged, not installed. Under that a dashed coral line with a single gap marks the boundary: staging is not installing, and the one way through is the rider's Select press. A green arrow passes through the gap into the device, which confirms, arms and installs.">
  <defs>
    <marker id="ota-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="ota-g" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#9aa884" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">Tag to glass — the automation delivers, the rider installs</text>

  <!-- the tag -->
  <rect class="d-panel" x="24" y="42" width="118" height="58" rx="10" />
  <text class="d-title" x="83" y="70" text-anchor="middle">v1.3.0</text>
  <text class="d-sub" x="83" y="88" text-anchor="middle">a pushed git tag</text>
  <line x1="144" y1="71" x2="180" y2="71" class="d-flow" marker-end="url(#ota-a)" />

  <!-- the workflow -->
  <rect class="d-panel-2" x="184" y="36" width="254" height="72" rx="12" style="fill:#f4ecd6" />
  <text class="d-label" x="311" y="56" text-anchor="middle" style="fill:#a9501c">release.yml</text>
  <text class="d-sub" x="311" y="74" text-anchor="middle">build → objcopy → wrap + SIGN</text>
  <text class="d-sub" x="311" y="89" text-anchor="middle">then inspect as the gate: the sig</text>
  <text class="d-sub" x="311" y="103" text-anchor="middle">must verify under this build's key</text>
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
  <text class="d-sub" x="360" y="250" text-anchor="middle" style="font-size:9px;fill:#a9501c">a GitHub release asset sends no CORS header — a browser fetch cannot read it</text>

  <!-- fan out -->
  <path d="M300 256 C 240 268, 180 272, 130 284" fill="none" class="d-flow" marker-end="url(#ota-a)" />
  <line x1="360" y1="256" x2="360" y2="284" class="d-flow" marker-end="url(#ota-a)" />
  <path d="M692 108 C 716 180, 700 250, 606 282" fill="none" stroke="#9aa884" stroke-width="1.3" stroke-dasharray="4 4" marker-end="url(#ota-g)" />
  <text class="d-sub" x="648" y="188" text-anchor="middle" style="font-size:9px;fill:#6b7758">from the</text>
  <text class="d-sub" x="648" y="201" text-anchor="middle" style="font-size:9px;fill:#6b7758">release page,</text>
  <text class="d-sub" x="648" y="214" text-anchor="middle" style="font-size:9px;fill:#6b7758">by hand</text>

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
  <text class="d-title" x="592" y="312" text-anchor="middle">any computer</text>
  <text class="d-sub" x="592" y="329" text-anchor="middle">copy UPDATE.BIN to the card</text>
  <text class="d-sub" x="592" y="343" text-anchor="middle">no manifest, no network</text>

  <path d="M128 348 C 128 360, 180 362, 212 369" fill="none" class="d-flow" marker-end="url(#ota-a)" />
  <line x1="360" y1="348" x2="360" y2="369" class="d-flow" marker-end="url(#ota-a)" />
  <path d="M592 348 C 592 360, 540 362, 508 369" fill="none" class="d-flow" marker-end="url(#ota-a)" />

  <!-- staged -->
  <rect class="d-panel-2" x="210" y="372" width="300" height="38" rx="9" style="fill:#f8efe4" />
  <text class="d-label" x="360" y="390" text-anchor="middle" style="fill:#a9501c">/UPDATE.BIN on the card</text>
  <text class="d-sub" x="360" y="404" text-anchor="middle">staged — not installed</text>

  <!-- the wall, with exactly one door -->
  <line x1="24" y1="436" x2="300" y2="436" class="d-hot" stroke-dasharray="6 5" />
  <line x1="420" y1="436" x2="696" y2="436" class="d-hot" stroke-dasharray="6 5" />
  <text class="d-sub" x="292" y="429" text-anchor="end" style="font-size:9.5px;fill:#a9501c">staging is not installing</text>
  <text class="d-sub" x="428" y="429" style="font-size:9.5px;fill:#3c6b39">one way through: the rider's Select press</text>
  <line x1="360" y1="410" x2="360" y2="452" class="d-flow" marker-end="url(#ota-a)" />

  <!-- the device -->
  <rect class="d-panel" x="250" y="454" width="220" height="46" rx="11" />
  <text class="d-title" x="360" y="476" text-anchor="middle">the device</text>
  <text class="d-sub" x="360" y="492" text-anchor="middle">confirms, arms, installs</text>
</svg>
<figcaption>One build, two homes, one pointer, three clients — and a wall with a single door in it. The workflow is the only thing that signs, and the <code>inspect</code> gate means it cannot publish a container its own firmware would refuse. The <b>release</b> is the archive; the <b>bucket</b> is only where the bytes are served, which is why the manifest names a URL instead of the clients knowing one. The dashed path is the manual one, and it is deliberately still there: it needs no manifest, no network and no app. Every path converges on a <em>staged</em> file, and the only thing that turns staged into installed is a rider pressing Select.</figcaption>
</figure>

## Three ways an update arrives

A staged update is one file, `/UPDATE.BIN`, in the card's root — an
[`OBCU`](src:specs/OBCU_Spec.md) container (a 64-byte header with the image length,
CRC-32, a version string — the release tag, for anything the pipeline published — and
the signature-scheme marker, then the raw application image, then the signature). There
are three ways it gets there, and
**exactly one** way it gets installed.

- **Card sideload.** Copy `UPDATE.BIN` onto the card from any computer, put the
  card back, and choose **Settings → System → Firmware → "Install update from card"**. The
  device scans and validates the file, shows what it found (the installed version
  → the staged version, plus a no-undo warning), and installs on a Select press.
  This is the primitive contract: a file on a card, nothing more. The row is
  disabled while a ride is recording, because arming reboots the device.
- **BLE from the companion.** The [companion app](../companion-link/) can stage the
  same `UPDATE.BIN` over the link: it uploads a `fwImage` object (§7.6 of the BLE
  spec), the device writes it to the card verbatim, and then the app sends an
  `installFw` command (§4.4) to *ask* the device to install it. The file is either
  one you picked in Files or the published release the app checks for — downloaded
  and verified against the manifest's size and SHA-256 on the phone, before a byte
  goes over the link. The app also *raises* a published update without being asked:
  a sheet the next time you open it, or a local notification from a periodic
  background check. Both are one switch away (on by default), both go quiet for a
  device whose running version isn't a release version, and both ask each version
  once — the check itself is an anonymous request for one public file.
- **USB from the browser.** The map builder's device step does the same two
  steps over the cable — the [object model is the transport's
  guest](../companion-link/), so this is the identical `fwImage` upload followed
  by the identical `installFw` request. It reads the running version from the
  Device Information service, compares it against the published release, and
  checks the container *before* spending the transfer on it: magic, header
  version, header CRC-32, the image CRC-32 over the bytes that follow, and the
  slot ceiling. That is the same decode rule the armer applies and it replaces
  nothing — the device still verifies over what actually landed on the card, and the
  *signature* check is the device's alone. What
  it buys is that "that isn't a firmware update" arrives in a second instead of
  after an upload. A device running a development build reports a git hash rather
  than a version, which does not parse: no update is ever offered for it, and the
  page says exactly that — *development build, automatic updates paused* — rather
  than going quiet. The file picker beside it still accepts an `UPDATE.BIN` by
  hand, which is how such a device gets back onto a release.

That check is one anonymous GET of a published manifest — no account, no query,
nothing said about the device — made only **once a device is connected**, because
with nothing to compare against the request would buy the rider nothing and cost
them a connection they did not ask for. The manifest is served from the project's
own domain, mirrored from the tagged GitHub release, which stays the source of
truth; nothing about trust rides on that choice, because what says an image is
genuine is the signature the device verifies before it will arm anything — not the
host that served it. The SHA-256 beside it answers a narrower question, and only
that one: whether the download arrived whole, checked here before a byte goes over
the cable. When the answer is *there is something newer*, the page says so
where the rider already is — a small note, never a modal, naming the version and
offering to show it. It stages nothing itself; it can only point at the one card
that does, and it asks once per device and release. A channel pointer moving
backward does not turn an older release into a new question; only a genuinely
newer version can raise the note again.

The crucial rule is shared by all three paths and stated plainly in the BLE spec's
security posture: **installing always confirms on the glass.** A peer can
*stage* an image — that is all a bonded link or a plugged-in cable authorises — but
it can never arm or reboot the device on its own. `installFw` merely posts a request; the
device runs its own scan and shows a **confirm card**, and the update proceeds only
on a physical Select press by the rider, exactly like the pairing-passkey pattern.
There are no silent installs, ever. The running firmware's version a peer
displays is read from the standard [DIS](../companion-link/) Firmware Revision
characteristic, so after a confirmed update it simply reflects the new image on the
next connect.

That characteristic answers with the version string of the **container the device
installed** — the `fw_version` the image was wrapped with, which for a released
build is the release tag. It is not the running build's own idea of its version,
and the difference is the whole point: a version a device made up from its source
tree can never equal a published release, so nothing could be compared to it. A
board flashed over a debug probe has installed no container at all, so it falls
back to reporting its git hash, which parses as no version — and that is the
locked answer, not a gap to close: an update is never offered against a build
nobody published. Such a device is updated the same way it was flashed, or by
staging a container by hand.

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

## The chain, layer by layer

Five things stand between a tagged build and a device running it, and the useful way to
read them is by the question each one answers — because each answers exactly one, and
none is a stand-in for another. Stacking checks that all answer the same question is how
a trust chain gets long without getting stronger.

- **HTTPS, on every fetch.** *Am I talking to the host I meant to?* That is all it
  answers: it authenticates a server, not a file, and says nothing about what the server
  chose to hand over. It is still load-bearing, because without it the manifest itself
  could be rewritten in flight — which is why the parsers refuse a `url` that is not
  `https`, so a manifest cannot downgrade its own download.
- **The Ed25519 signature, verified on the device.** *Are these bytes ours?* This is the
  only layer that answers it, and deliberately the only one that has to be trusted: it
  is checked by the firmware, against a key compiled into that same firmware, over the
  bytes that actually landed on the card. Everything upstream of it is convenience;
  this is the one that would have to be broken.
- **The manifest's SHA-256, checked on the phone or in the browser.** *Is this the file
  the manifest described?* A download is measured — byte count first, then the digest —
  before a single byte is sent to the device. It is emphatically *not* a second opinion
  about authenticity: the digest and the image come from the same place, so a host able
  to swap one could swap both. Its job is that a truncated or wrong download fails in a
  second, on a machine with a screen and a fast link to retry on, instead of after a
  multi-minute BLE transfer and a rejection card on the glass.
- **The container's CRC-32, checked by the bootloader.** *Did the bytes survive the
  card?* This covers the stretch nothing else watches: the write to the SD card, the card
  spending a week in a frame bag, the block reads during the install. It is also the only
  check the bootloader can make without a key, which is what makes verify-before-erase
  possible at all.
- **The confirm press, then the trial boot.** *Does the rider want this, and does it
  work?* No automation reaches either. A peer can only ever leave a file on the card; the
  device does its own scan and asks on the glass, and afterwards the new image has to
  prove itself once before it becomes permanent — otherwise the snapshot comes back.

Read downwards the chain narrows: transport, then provenance, then a particular file,
then particular bytes, then a person. Read as a set of failures it is easier to check.
A tampered transport is caught by the signature. Without the signing key, a hostile
serving host cannot produce a container the device will install, only refuse to serve
one. A forged image with a
perfect CRC dies at the signature; a genuine image with a broken CRC dies before the
erase. A signed, intact, genuinely-newer image still does nothing until someone presses
Select — and if it then fails its trial boot, the snapshot comes back. These checks
bound delivery and install failures; they do not claim that an authorized, signed
release is free of application bugs, which remains the job of release testing.

## The RRAM layout

The design fits in the device's non-volatile RRAM as a fixed partition the small
bootloader and the big application both agree on. The app is *always* linked at
`0x8000` — there is one build shape, no bootloader-less variant — so dev flashing
is "flash `obc-boot` once, then iterate on the app exactly as before".

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="A memory map of the device's RRAM as a horizontal bar, and the SD card beside it. The RRAM bar has five segments left to right: obc-boot, the 32-kilobyte bootloader at address 0x0; a large app slot of 1976 kilobytes starting at 0x8000; a 20-kilobyte SEMMC_STAGE carve at 0x1F6000 holding the staged sEMMC soft-peripheral image; a 4-kilobyte BOOT_STATE page at 0x1FB000; and a 4-kilobyte SETTINGS page at 0x1FC000. Below the segments their start addresses are labelled. To the right, a smaller SD card panel lists two files in the card root: UPDATE.BIN, the staged OBCU container, and ROLLBACK.BIN, the snapshot of the running image written by the armer.">
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
  <text class="d-label" x="652" y="94" text-anchor="middle">SD card root</text>
  <rect class="d-panel-2" x="616" y="108" width="72" height="42" rx="7" style="fill:#f8efe4" />
  <text class="d-sub" x="652" y="126" text-anchor="middle" style="fill:#a9501c">UPDATE</text>
  <text class="d-sub" x="652" y="140" text-anchor="middle" style="fill:#a9501c">.BIN</text>
  <rect class="d-panel-2" x="616" y="160" width="72" height="42" rx="7" />
  <text class="d-sub" x="652" y="178" text-anchor="middle">ROLLBACK</text>
  <text class="d-sub" x="652" y="192" text-anchor="middle">.BIN</text>
</svg>
<figcaption>The bootloader lives in its own 32 KB slot below the app and is flashed once; the app never moves it. <code>BOOT_STATE</code> is the single 4 KB handoff page — the armer writes an <code>Armed</code> record there and the bootloader reads it, both through the shared codec, and any unclean read is <code>Idle</code>. <code>SEMMC_STAGE</code> is the second, bulkier handoff: since the storage pivot the card is only reachable through the sEMMC soft peripheral — a ~13.6 KB coprocessor image the 32 KB bootloader cannot afford to embed — so the armer copies the app's image into this CRC-framed carve before every arm, and the bootloader validates and boots the card through it (<code>OBCU_Spec.md</code> §3). The staged <code>UPDATE.BIN</code> and the <code>ROLLBACK.BIN</code> snapshot live on the card, not in RRAM: the app resolves them to raw SD block runs so the FAT-free bootloader can read them with plain block reads, no filesystem.</figcaption>
</figure>

The bootloader is deliberately tiny and dumb — no FAT, no BLE, no display driver,
no async executor, just blocking GPIO + a block-read transport + RRAMC. All the logic that could be
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

- The byte formats — the `UPDATE.BIN` container, the boot-state page, and the blob-stage carve — normative: [`OBCU_Spec.md`](src:specs/OBCU_Spec.md)
- The shared `no_std` core: [`obc-dfu`](src:firmware/obc-dfu) — the container + state codecs ([`image.rs`](src:firmware/obc-dfu/src/image.rs) · [`state.rs`](src:firmware/obc-dfu/src/state.rs)), the bootloader's install engine ([`engine.rs`](src:firmware/obc-dfu/src/engine.rs)), and the app-side armer ([`armer.rs`](src:firmware/obc-dfu/src/armer.rs))
- The bootloader itself, its LED codes and flash-once workflow: [`obc-boot`](src:firmware/obc-boot) ([README](src:firmware/obc-boot/README.md))
- The host tool that builds and inspects `UPDATE.BIN`: [`obc-mkimage`](src:host/obc-mkimage) — the `objcopy → wrap` pipeline is in the [firmware README](src:firmware/README.md)
- The publish pipeline — the tag trigger, the signing and `inspect` gate, the release assets and the R2 mirror: [`release.yml`](src:.github/workflows/release.yml); the signing key and its rotation recipe: [`obc-dfu/keys/README.md`](src:firmware/obc-dfu/keys/README.md)
- The manifest readers and the shared version dialect: [`builder/app/src/lib/firmware/release.ts`](src:builder/app/src/lib/firmware/release.ts) for the web and desktop hosts, and its Swift twin in [`OBCKit/Sources/OBCTransport/Firmware/`](src:companion-ios/Packages/OBCKit/Sources/OBCTransport/Firmware) for the phone
- The BLE and USB staging paths — the `fwImage` object and `installFw` command: [the companion link](../companion-link/) (contract: [`obc-ble-interface-spec.md`](src:specs/obc-ble-interface-spec.md) §4.4, §7.6); the browser half in [`web_builder/frontend/src/lib/device/`](src:builder/app/src/lib/device)
- Copying a card image by hand and the release recipe: the [repo README](src:README.md)
