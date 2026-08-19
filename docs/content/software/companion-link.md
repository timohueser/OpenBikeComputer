---
title: The companion link
description: How the OpenBikeComputer device and its phone companion app talk over Bluetooth Low Energy — the two-plane GATT / L2CAP split, the object model, transfers, the change-signal sync loop, store epochs, and passkey pairing.
---

# The companion link

> This page describes the currently implemented legacy link while the coordinated **flat store**
> cutover is under development. The replacement contract is
> [`FLAT_Store_Protocol.md`](src:specs/FLAT_Store_Protocol.md) — wire major **4**, six
> opcodes (`LIST`, `STATUS`, `GET`, `PUT`, `REMOVE`, `CANCEL`) plus `ARM` for a firmware install,
> one transfer at a time, no resume and no operation ids: the card's catalog *is* the result, and
> [`FLAT_Store_Format.md`](src:specs/FLAT_Store_Format.md) is what that catalog is made of. (An
> earlier Device Object System v2 / wire-major-3 design once stood here; it was superseded before
> it shipped and its specs are tombstoned.) There is no compatibility or dual-write path between
> the two designs.

The device is a self-contained navigator, but a route is usually *planned* on a
phone and a ride is worth keeping once it's ridden. A small **iOS companion app**
bridges the two over **Bluetooth Low Energy**: push a planned route to the
device, pull tracked rides back, rename the device, read its diagnostics. Once
you've paired, powered, and are in range, it just works — no accounts, no cloud,
nothing leaves the two devices.

This page is the *shape* of that link. The normative, byte-level reference **for the legacy link
described here** is the [BLE interface spec](src:specs/obc-ble-interface-spec.md) (the same tier as
the [`OBCM`](src:specs/OBCM_Spec.md) / [`OBCR`](src:specs/OBCR_Spec.md) format specs); for the flat
store the normative reference is instead the pair
[`FLAT_Store_Protocol.md`](src:specs/FLAT_Store_Protocol.md) (the seam and the wire) and
[`FLAT_Store_Format.md`](src:specs/FLAT_Store_Format.md) (the card). Here we
cover the design and the *why*. Five ideas run through all of it:

- **Two planes.** Small typed control state rides GATT; bulk bytes ride a single
  L2CAP channel. Nothing large ever crosses GATT.
- **Objects are files the device already speaks.** A route crosses the wire as
  an [OBCR](../formats/) file and is written to storage verbatim; the phone does
  every format conversion, so **the device never parses XML**.
- **One CRC, end to end.** A whole-object checksum is verified once, at commit —
  the check the on-air link CRC can't give you.
- **Interrupted transfers restart, not resume.** Objects are small enough that
  re-sending one whole is simpler and safer than continuing from an offset.
- **One device → app channel.** Every message the device sends back — a transfer
  result, a store-change signal, a download's announce — rides a single `status`
  notify characteristic, so there is one subscription and one ordering domain.

## Two planes: control and data

A BLE **GATT attribute is capped at 512 bytes** — a hard wall, not a soft
budget. A route is tens of kilobytes. So the link is split in two: GATT carries
the small, typed *control* state (identity, config, the orchestration of a
transfer, notifications), and a single **L2CAP connection-oriented channel
(CoC)** is the bulk *data* pipe. GATT says *what is about to happen and how it
went*; the CoC carries *the bytes*.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="The link split into two planes between the companion app on the left (BLE central) and the OBC device on the right (BLE peripheral). The top lane is the control plane over GATT — six small typed characteristics: command, status, config, transferControl, psm, protocolVersion — with a note that each attribute is capped at 512 bytes. The bottom lane is the data plane over a single L2CAP connection-oriented channel: one raw byte pipe with credit-based flow control, carrying bulk objects one transfer at a time.">
  <defs>
    <marker id="cl-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">Two planes — control on GATT, bulk on L2CAP</text>

  <!-- phone -->
  <rect class="d-panel" x="16" y="88" width="120" height="180" rx="12" />
  <text class="d-title" x="76" y="164" text-anchor="middle">companion app</text>
  <text class="d-sub" x="76" y="186" text-anchor="middle">BLE central</text>
  <text class="d-sub" x="76" y="202" text-anchor="middle">(iPhone)</text>

  <!-- device -->
  <rect class="d-panel" x="584" y="88" width="120" height="180" rx="12" />
  <text class="d-title" x="644" y="164" text-anchor="middle">OBC device</text>
  <text class="d-sub" x="644" y="186" text-anchor="middle">BLE peripheral</text>
  <text class="d-sub" x="644" y="202" text-anchor="middle">nRF54L</text>

  <!-- control lane -->
  <rect class="d-panel-2" x="150" y="88" width="420" height="86" rx="10" style="fill:#eef2df" />
  <text class="d-label" x="360" y="112" text-anchor="middle" style="fill:#3c6b39">Control plane · GATT</text>
  <text class="d-sub" x="360" y="132" text-anchor="middle">small, typed state — identity · config · orchestration</text>
  <text class="d-sub" x="360" y="150" text-anchor="middle" style="font-size:9.5px">command · status · config · transferControl · psm · protocolVersion</text>
  <text class="d-sub" x="360" y="168" text-anchor="middle" style="fill:#a9501c">≤ 512 bytes per attribute — a hard wall</text>

  <!-- data lane -->
  <rect class="d-panel-2" x="150" y="190" width="420" height="78" rx="10" />
  <text class="d-label" x="360" y="216" text-anchor="middle" style="fill:#33575b">Data plane · L2CAP CoC</text>
  <text class="d-sub" x="360" y="236" text-anchor="middle">one raw byte pipe · credit-based flow control</text>
  <text class="d-sub" x="360" y="254" text-anchor="middle">bulk objects, one transfer at a time</text>

  <!-- connectors -->
  <line class="d-flow" x1="136" y1="131" x2="150" y2="131" marker-start="url(#cl-a)" marker-end="url(#cl-a)" />
  <line class="d-flow" x1="136" y1="229" x2="150" y2="229" marker-start="url(#cl-a)" marker-end="url(#cl-a)" />
  <line class="d-flow" x1="570" y1="131" x2="584" y2="131" marker-start="url(#cl-a)" marker-end="url(#cl-a)" />
  <line class="d-flow" x1="570" y1="229" x2="584" y2="229" marker-start="url(#cl-a)" marker-end="url(#cl-a)" />
</svg>
<figcaption>The <b>control plane</b> is GATT: three services — Device Information and Battery (both standard SIG services) plus a custom <b>OBC Control</b> service whose characteristics orchestrate everything. The <b>data plane</b> is a single L2CAP CoC — a reliable, ordered byte pipe. The one object small enough to live on GATT is the <code>config</code> blob, so renaming the device is a plain characteristic write; everything bigger goes over the CoC.</figcaption>
</figure>

**The CoC is a raw byte pipe — deliberately.** The BLE Link Layer already CRCs
and retransmits every packet, so the channel is reliable and ordered. That means
bulk transfer needs **no per-chunk framing**: a control-plane descriptor
announces the transfer, the CoC then carries *exactly* the object's payload
bytes, and the device sinks them straight to storage while updating a running
checksum. There is **no reassembly buffer** — which is the whole point on a
RAM-limited microcontroller.

It also buys something the design didn't set out to get. A channel with no
framing of its own makes **no demands on what carries it** beyond "reliable and
ordered" — and a USB bulk endpoint is exactly that. So the object model above,
the 12-byte descriptor, the status envelope and the whole-object CRC-32 all
transplant onto a cable without a byte changing: USB is a second *transport*, not
a second protocol, and it reads the same
[`specs/vectors/`](src:specs/vectors) fixtures. The host half lives in the
web builder's [USB client](src:builder/app/src/lib/usb), which
drives the whole contract over an in-memory device; the device half — the LM20's
USB peripheral, and the small matter of which control characteristic a frame
belongs to when there is no GATT to say so — is
[`obc-fw-nrf54l/src/usb/`](src:firmware/obc-fw-nrf54l/src/usb), and it ships in
**every** firmware build rather than behind a flag. The plane now comes up on
hardware, and the fix that got it there is worth knowing about: it must be built
*when a cable appears*, never at boot — see below.

That host client has **two** transports under it, and only one of them is a
browser. WebUSB is Chromium-only, so the desktop app drives the cable itself
through [`nusb`](src:apps/obc-desktop/src/usb) and is the universal path,
including for Safari and Firefox. Everything above the byte pipe is the same
code: the same object model, the same descriptors, the same CRC, the same
fixtures. The native side moves bytes and nothing else — it does not know what an
object id is — which is the property that keeps one protocol implementation
rather than two drifting ones.

There is one place the two hosts genuinely differ, and it is about size. A
browser holds a map in a scratch file and streams it through the tab; the desktop
app hands the *file path* to the transport and the bytes go disk → endpoint
without ever entering the webview — no reason to copy a few hundred megabytes
through JavaScript just to hand them straight back.

The browser side already **writes** over it: a map, a dropped GPX and a firmware
image, from the [map builder](https://openbikecomputer.com/)'s
device step. The cable changes exactly one thing about the object set, and it is
the interesting one. A **map** was never an object, because a 200 MB file was
never going over BLE — so USB adds a `map` type carrying an
[OBCM](../formats/) file, uploaded and committed by the same six steps as
everything else. Two consequences follow from the size rather than from the
format: the browser cannot hold the artifact (it streams the download into a
scratch file, checksums it against the [catalog
manifest](src:specs/OBCC_Spec.md) and only then opens the transfer, because the
descriptor has to announce a whole-object CRC before the first byte moves), and
the transfer is still measured in *tens of seconds* — a few hundred megabytes is
a few hundred megabytes whichever end is slower. Which end that is has actually
changed: the card was the obvious ceiling while it was reached over SPI, and
since the storage transport moved to native 4-bit sEMMC it writes at 8.2 MB/s raw,
which is no longer obviously below what the cable delivers. What sits between the
two is filesystem bookkeeping and how much of the pipeline can overlap — see the
pipeline notes further down. How maps are named and enumerated on the card is the
device side's answer to the same size, and it is worth its own section below.

## Objects are files the device already speaks

Every bulk payload is a typed **object**. The set is small and closed:

| `type` | Object | Direction | Payload |
|--------|--------|-----------|---------|
| `1` | `route` | app → device (upload) · device → app (detail read) | an [OBCR](../formats/) route file, verbatim |
| `2` | `ride` | device → app | the compact [ride object](../formats/#recorded-rides-the-track-log-and-the-ride-object) (a tracked ride) — **v1**, or **v2** when it carries recorded sensor data |
| `4` | `diagnostics` | device → app | an opaque text blob (boot count, link + storage counters, stack high-water…) |
| `6` / `7` | `routeList` / `rideList` | device → app | the store catalogs — fixed-size entries (`routeList` **84 B**, `rideList` **72 B**) |
| `9` | `trip` | app → device (upload) · device → app (detail read) | a **trip** — tiny metadata that *references* member routes by object id in ride order (spec §7.7); routes stay standalone OBCR files |
| `10` | `tripList` | device → app | the trip catalog — fixed-size **76 B** entries, mirroring `routeList`'s core (no auto-expiry tail) |
| `5` | `fwImage` | app → device (upload) | a firmware update image — an [`OBCU`](src:specs/OBCU_Spec.md) `UPDATE.BIN` container, staged to the card verbatim (see below) |
| `3` | `config` | — | reserved on the CoC; the Config blob crosses GATT |
| `16` | `map` | host → device (upload) | an [OBCM](../formats/) map — **the cable only**, see below |

`map` is the one type Bluetooth could never have carried, and it is the clearest
illustration of what a second transport buys: a map is hundreds of megabytes, so
the type would have been dead weight until [a wire existed](#the-same-link-down-a-cable)
that could move one.

### A map is the one object that does not fit the pattern

Every other upload gets its atomicity the same way: the bytes stream into a temp
file the catalog scans never match, and the commit *copies* that temp to its
final name, holding the 4-byte format magic back until the body is durable. A
power cut leaves either an invisible temp or a magic-less file every reader
rejects. Nothing half-written is ever visible.

A map cannot pay for that copy. At a few hundred megabytes it would double both
the write time — already minutes — and the free space the card must have, to buy
a guarantee for the one object that can always be built again. So a map streams
**straight into its final file**, and earns the same commit point a different
way: the file opens with four zero bytes where the magic goes, the stream's own
first four bytes are held aside, and they are written last, after the
whole-object checksum *and* the header have both checked out. The interrupted
state is byte-for-byte the one the copy leaves — a magic-less file the map
catalog refuses and a boot sweep reclaims.

Five more rules fall out of the same size:

- **A map upload is new-only.** Writing into a stored map's file would destroy it
  as the replacement arrives, and "a failed checksum never touches the old copy"
  is not a promise to break on the one file the rider needs to see where they
  are. Replacing a map is *send the new one and let the device retire the old
  one*; there is no delete for a map on the wire at all.
- **The device keeps one uploaded map.** It loads a single map and never switches
  between them — the choice is made once at startup and the file stays open for
  the session — so a second copy is a few hundred megabytes no reader will ever
  open. The retirement happens at the boot that adopts the new map, and only once
  that map has opened: the upload lands while its predecessor is still being
  streamed from, so the instant of the commit is exactly when the old file cannot
  be touched. Between the two, the card carries both. A map the rider copied on
  themselves is never retired — it has no device-assigned id, and the rule is one
  *uploaded* map, not one file.
- **Free space is checked before the first byte**, not discovered at the last.
  A card that cannot fit the announced map is told so at the announce, with a
  reserve left over so a map can never take the last cluster and strand the ride
  log.
- **The device knows the map arrived, but not what it is.** The descriptor has no
  name field, the payload is opaque bytes, and the [OBCM header](../formats/)
  carries no name and no build date. A device can list the maps it holds — id,
  filename, size, format version, bounding box, all read off the card — and
  cannot say where any of them came from. That is a gap in the protocol, not in
  the filesystem, and closing it means a new command rather than a new object.
- **A break costs the whole map.** The link's [restart-not-resume
  rule](#a-transfer-end-to-end) is free for an object measured in kilobytes and
  expensive for one measured in gigabytes: a cable pulled at ninety per cent
  starts again at zero. It is accepted rather than engineered around, because
  resuming means the device can say which prefix of which map it already holds
  *and prove it*, and a whole-object checksum cannot check a suffix. What bounds
  the cost is the wire, not the rule — a country-scale map is about twenty
  minutes over the cable, so the worst case is a second twenty minutes, not a
  lost afternoon.

Because the FAT layer the firmware uses creates 8.3 filenames only, a received
map lands as `MP7.OBM` — the same trick the [reserved computed-route
file](../architecture/#on-device-routing-the-router-seam) plays with `.OBR`, and the
same convention that puts a stored object's durable id in its filename. Which map
the renderer streams from
is recorded in a small file beside them, and a map that has just arrived becomes
that choice; it takes effect at the next start-up, because the map's tables are
parsed once at boot and held for the whole session.

The key move is that **a route on the wire is the same bytes as a route on the
card.** The phone converts an imported GPX or TCX to an OBCR file and streams
that; the device writes it to storage byte-for-byte and later serves it back the
same way. There is no separate "detail" codec — the app's route-detail screen
decodes the very OBCR bytes it uploaded. One layout, one truth. A tracked ride
is the mirror in the other direction: the device stores each finished ride as
the exact bytes it will later stream, so a ride download is a verbatim file copy.

One object is not a stored file but a **firmware update**: a `fwImage` upload
carries an [`OBCU`](src:specs/OBCU_Spec.md) `UPDATE.BIN` container, which the device
writes to the card root verbatim — the transfer layer stays format-blind, exactly
as with a route's OBCR bytes. That container is **signed**, and the *device* checks the
signature before it will install anything, so a peer can only ever stage an image it
obtained from a real release. **Staging is not installing.** A committed `fwImage`
only *places* the file; the app then sends a separate `installFw` command to
*request* an install, and the device runs its own scan and shows a **confirm card**
that the rider must approve with a physical Select press. The phone can never arm
or reboot the device on its own — the same on-glass gate the pairing passkey uses.
The whole trust model, the three delivery paths, how a tagged release is published and
served so the phone can find it at all, and the RRAM layout are on the
[firmware updates](../firmware-updates/) page.

**Object ids are durable.** Each stored object has a `u16` id the device assigns
and keeps **stable across reboots** — the reference firmware encodes it right in
the filename (`RT{id}.OBR` for routes, `RD{id}.ORD` for rides). Durability is
what lets the phone remember *"I uploaded route 7"* and later ask *"is 7 still
there?"* or replace it in place — and it's what a ride sync's
already-have-this-one set keys on. That promise holds **within an id era**; when
the id space itself resets, a **store epoch** names the new era so the phone never
mistakes a reused id for the old object — see [Store epochs](#store-epochs-which-id-era-youre-talking-to).

## A transfer, end to end

Every bulk exchange is the same three-beat shape: **announce over GATT, stream
over the CoC, confirm over GATT.** Here is an upload — a route leaving the phone:

<figure class="fig">
<svg viewBox="0 0 720 372" role="img" aria-label="A sequence diagram of an upload between the companion app on the left and the OBC device on the right. Step one: the app writes a 12-byte transferControl descriptor over GATT — op equals upload, plus type, object id, total length and CRC-32 — which announces the transfer. Step two: the app streams the object's raw bytes over the L2CAP CoC. On the device side a note reads: sink to storage, updating a running CRC, with no reassembly buffer. Step three: once all bytes are in, the device verifies the whole-object CRC. Step four: the device notifies a transferResult over GATT — committed on a match, or crcMismatch which rejects the object so nothing is stored.">
  <defs>
    <marker id="tf-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="tf-c" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7.5" markerHeight="7.5" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">One upload — announce · stream · confirm</text>

  <!-- actors -->
  <rect class="d-panel" x="80" y="40" width="140" height="34" rx="9" />
  <text class="d-title" x="150" y="62" text-anchor="middle">companion app</text>
  <rect class="d-panel" x="500" y="40" width="140" height="34" rx="9" />
  <text class="d-title" x="570" y="62" text-anchor="middle">OBC device</text>

  <!-- lifelines -->
  <line x1="150" y1="74" x2="150" y2="352" style="stroke:#9aa884;stroke-width:1.2;stroke-dasharray:4 4" />
  <line x1="570" y1="74" x2="570" y2="352" style="stroke:#9aa884;stroke-width:1.2;stroke-dasharray:4 4" />

  <!-- 1: descriptor -->
  <text class="d-sub" x="360" y="104" text-anchor="middle" style="fill:#3c6b39">1 · transferControl (12 B) — GATT write (write-only)</text>
  <text class="d-sub" x="360" y="120" text-anchor="middle">op=upload · type · object_id · total_len · crc32</text>
  <line class="d-flow" x1="150" y1="130" x2="570" y2="130" marker-end="url(#tf-a)" />

  <!-- 2: stream -->
  <text class="d-sub" x="360" y="164" text-anchor="middle" style="fill:#33575b">2 · the object's bytes — L2CAP CoC, raw stream</text>
  <line x1="150" y1="176" x2="565" y2="176" style="stroke:#33575b;stroke-width:6;opacity:0.5" marker-end="url(#tf-a)" />

  <!-- device note -->
  <rect class="d-panel-2" x="404" y="196" width="230" height="46" rx="9" style="fill:#f7f4e6" />
  <text class="d-sub" x="519" y="216" text-anchor="middle">sink → storage, running CRC</text>
  <text class="d-sub" x="519" y="232" text-anchor="middle" style="fill:#a9501c">no reassembly buffer</text>

  <!-- 3: verify -->
  <rect class="d-hot" x="470" y="258" width="200" height="42" rx="9" style="fill:#f8efe4" />
  <text class="d-sub" x="570" y="278" text-anchor="middle">3 · all bytes in →</text>
  <text class="d-sub" x="570" y="294" text-anchor="middle">verify whole-object CRC-32</text>

  <!-- 4: result -->
  <line class="d-hot" x1="570" y1="322" x2="150" y2="322" marker-end="url(#tf-c)" />
  <text class="d-sub" x="360" y="342" text-anchor="middle" style="fill:#a9501c">4 · transferResult — GATT notify: committed  ·  (mismatch → rejected, nothing stored)</text>
</svg>
<figcaption>The descriptor names the transfer (and carries the whole-object CRC); the CoC carries the payload; the <code>transferResult</code> closes it. <code>transferControl</code> is <b>write-only</b> — the app writes it to <em>open</em> a transfer, the device never notifies it. A fresh upload sends object id <code>0xFFFF</code> ("new") and the device reports the <b>assigned</b> id in the result. A <b>download</b> is the mirror: the app asks with an <code>op=download</code> descriptor, and the device <em>answers</em> on the <code>status</code> channel with a <b>download announce</b> (a <code>status</code> message carrying the same descriptor, now with the size + CRC filled in), then streams the object back. Routing the announce through <code>status</code> is what keeps every device → app message on one characteristic.</figcaption>
</figure>

The checksum is a **whole-object CRC-32/IEEE**, verified once at commit — the
same variant as gzip/PNG. It is deliberately *not* a per-packet CRC (the link
already covers the air); it catches what the link can't — an encode bug, a
storage error — **end to end**, from the phone's request to the device's flash
and back.

> **Restart, not resume.** An object is tens of kilobytes — a couple of seconds
> on the wire — so a dropped or aborted transfer is simply re-sent (or
> re-requested) *whole*, never continued from a durable offset. The device
> discards a partial upload the moment the link drops or an `abort` arrives, and
> the app re-sends from byte zero. (A suffix couldn't be checked against the
> whole-object CRC anyway — which is why the descriptor carries no resume offset
> at all; the field v1 kept permanently zero is gone in v2.) A multi-object flow
> — syncing several rides — resumes
> at **whole-object granularity**: the rides that fully landed are kept, and the
> rest re-send from byte zero.

> **Full means full — up front.** The device holds a bounded route catalog (64
> routes). A **new**-route upload that would overflow it is refused the instant the
> descriptor arrives — *before the device consumes any payload* — with a distinct `storageFull`
> result, so the phone can tell the rider to delete routes on the device rather
> than wait out a doomed transfer. Since the raw sender may already have queued
> bytes before that asynchronous result arrives, the app resets the CoC on the
> reject. Re-uploading an *existing* route (a replace by
> id) is exempt: it reuses a slot rather than growing the catalog, so updating the
> route you're actively navigating never hits the cap.

> **Retries converge — never twin.** Restart-not-resume is only safe if the first
> attempt can't half-count, and there is one window where it could: the device
> commits an upload, then the link dies before the phone hears the `committed`
> result. The phone, none the wiser, re-sends the object as *new* — and without a
> guard the device would mint a same-content twin. So a **fresh** upload whose
> whole-object CRC (and length) match an object the device already stores answers
> `committed` with the **existing** object's id, storing nothing: a lost ack costs
> one re-send, never a duplicate. The phone closes the same window from its side
> without re-sending a byte — on every catalog reconcile, an *unlinked* entry
> whose `crc32` matches a library route's (or trip's) current encoding is
> **adopted** as that object's device copy, and a whole-trip upload re-reads both
> catalogs right before planning, so a retry sees what actually landed.

> **Answers are bounded — a lost notify never wedges.** Every solicited answer
> the app waits on rides the `status` notify, and the device deliberately
> **abandons** a notify it can't deliver in time rather than stall a plane — a
> lost notification is the app's to recover by re-reading. The app holds the
> same posture: each wait is time-bounded, because it holds the app's single
> transfer slot, and an unbounded wait on one lost verdict would wedge every
> later list read, sync, and upload behind it. On a timeout, cancellation, or
> reject the app closes and reopens the CoC (the channel is unframed, so a
> reset is what discards queued bytes), and the device treats the drop as an
> implicit abort. A committed close is correlated by object id **and byte
> count**, so the close of a preceding catalog read can never complete an
> upload; a data-plane stall under a live link is failed by a watchdog and
> surfaces as a plain retryable failure, never a progress bar parked at 99 %.

### Abort means two things, and neither of them deletes

Cancelling is the obvious use of `abort`, and not the common one. The other is
**quiescing**: after a transfer the device has already refused — a rejected
descriptor, an object whose checksum did not match — the host sends an abort not
to stop anything but to get the channel *empty* before it retries. On an unframed
pipe the sender does not wait between chunks, so bytes it queued for the refused
transfer are still arriving, and neither end can recall them; land them in front
of the retry and the retry fails a checksum for reasons nothing in the exchange
explains. The abort handshake is the one moment both ends are synchronised — the
host has stopped and is waiting for an answer — so it is the one moment the
device can read the channel dry.

Which of the two an abort is, the device decides from what is in flight: with a
transfer armed it discards that transfer's partial, and with nothing armed it
drains the channel and stops there. Either way it touches nothing already stored,
and that is a rule with no exception — an abort discards at most the bytes of the
transfer it interrupted, and can never reach an object the rider already has.
That is what lets the host send one as freely as a retry needs it: the drain is a
routine part of failing a transfer, not a decision about what to keep.

### What actually limits an upload

The protocol is not the limit and never has been: the bulk channel carries the
object's bytes with **no per-chunk framing and no per-chunk acknowledgement**, so
nothing in the exchange makes the host wait for the device between chunks. What
makes a map take minutes is the pipeline underneath, and it has four distinct
stages:

- **How much the host keeps queued.** A browser hands each write to the USB
  service and back, so with a single transfer outstanding the wire is idle for
  that round trip between every chunk. The host keeps a small bounded window of
  transfers queued instead — bounded, because the promise settling *is* the
  backpressure that stops a 300 MB map being read into the tab faster than the
  device can take it.
- **How the device receives.** A USB controller only accepts what the firmware
  has told it to expect, and the obvious instruction — *expect one packet* — is
  the expensive one: the endpoint then refuses everything after that packet until
  the firmware has been scheduled, copied it out and asked for the next. That
  refusal is not free, it is a round trip through an interrupt, a wake-up and the
  task scheduler, per 512 bytes, and it was the largest single term in the budget
  when this was first measured. The device now arms the endpoint for a **burst**
  of packets instead, so the controller keeps taking the wire into its own
  buffers while the processor is busy with the card, and the firmware collects
  the whole burst in one go. Buffer DMA also moves that burst between USB SRAM
  and memory without an interrupt and CPU copy per packet. The dial is a single
  constant; what it buys is that the stage below can run without stopping the
  wire.
- **How much reaches the card per command.** Handing the filesystem 512 bytes
  makes it issue one single-block write — one whole internal program cycle of the
  card — per packet. The firmware therefore stages arriving bytes in two **64 KiB
  halves**, coalesces their adjacent FAT clusters, and hands the card one
  128-block multi-write. One half fills from the cable while FLPR DMA writes the
  other; ownership does not return to USB until that DMA has completed.
- **Filesystem bookkeeping.** Extending a file by one cluster costs several
  single-block writes of the allocation table — and they land *between* the bulk
  bursts, which is exactly where they hurt. An upload announces its length before
  the first byte, so the whole chain can be reserved up front. A one-sector FAT
  cache then absorbs the residual updates, and the card gets a best-effort
  pre-erase hint before each long run.

The result measured on the LM20-DK is roughly **7.3–7.9 MB/s** for real builder
output, versus about 2.0 MB/s for the original synchronous path with a software
CRC pass. At that point the card and filesystem are again the visible ceiling,
not USB framing or checksum work.

### When a route lands — the device's side

A committed upload isn't silent on the device. A route usually arrives because
the rider just pressed *send* on the phone sitting next to it, so the display
**wakes** and shows a short prompt — then returns to warm sleep. The prompt is
strictly **advisory**: the route is already in the store (and the Route menu)
before it appears, so dismissing it loses nothing. It **auto-closes after 30
seconds**, and that timeout *is* a dismiss. What the prompt offers depends on
what the rider is doing:

- **Not riding** → *"Route received — View route / Dismiss."* The card shows the
  route's name, its distance/climb, and a mini elevation sparkline; *View route*
  opens the same **Route overview** picking the route in the Route menu opens
  (where START RIDE is one press away) — it never starts a ride directly.
- **Riding** → the same guarded **swap** shape a mid-ride route pick uses (*Swap
  route / Finish &amp; new / Cancel*), retitled for a received route and carrying the
  route's distance/climb — so an uploaded route mid-ride can't silently take over
  navigation.
- **Replacing the route you're navigating** → an **info-only** card. The device
  has no choice here: the replace-commit already overwrote the file on the card,
  so the old bytes are gone. The device *adopts the new version immediately* —
  it reopens the geometry handle, re-runs map-matching from the current fix, and
  recomputes progress — and the card just tells the rider it happened. The
  recording session is untouched.
- **A whole trip** → one *"Trip received — View trip / Dismiss"* card. The member
  routes commit first (each raising the prompt above, the newest replacing the
  last), then the trip object lands and its card takes the prompt's place — so
  the transfer ends on a single card showing the trip's name, summed
  distance/climb, and stage count; *View trip* opens the trip's folder in the
  Route menu. Same card whether idle or riding (a trip is a folder, not a
  navigable route — there is nothing to swap onto).

Two rules keep the prompt from ever doing harm. It **never lands while a hold
gesture is charging** — a popup appearing under a half-completed *Finish &amp; new*
hold could complete onto the wrong action, so it waits a tick (the same
stack-change hold-cancel the [UI page](../ui/#hold-to-confirm) describes).
And consecutive uploads **replace** the prompt rather than stacking — most
recent wins, carried by object id, not menu position, so a live rescan can't
point *View route* at whatever route slid into the slot. A pending prompt
is also **outranked** by the passkey card: if pairing starts, the route prompt
is dropped (not queued) — it's only advisory, and the route is safe in the menu.

## Staying in sync — the change signal

After anything changes on the device — a route uploaded, a ride finished, an
object deleted — the phone needs to know *what to re-fetch*, cheaply. Re-reading
the full catalogs on every reconnect would burn the CoC for nothing. So the
device fires a tiny **`storeChanged`** message on the `status` channel: a byte
naming *which* store moved (route or ride) plus a `revision` counter bumped on
every change to it. It is the **sole** change signal — one notification says
*"the ride store moved; re-list it"*, and the app reads nothing else to learn
something changed.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="The sync loop as four stages left to right. One: a store change on the device — an upload from the phone commits, a ride is tracked, or the rider deletes a route or ride on the device itself; every path goes through the one object store. Two: the device fires a storeChanged message on the status channel, naming which store moved and bumping its revision. Three: on that signal the app downloads the relevant list object — routeList or rideList — over the CoC. Four: the app fetches only the objects that actually changed. A curved arrow returns from stage four to stage one, labelled on the next change, showing the loop. Below, a separate reverse lane: on every connect the phone sends an ackRides command back to the device — the ids it holds — and the device marks those rides synced.">
  <defs>
    <marker id="sy-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="sy-m" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#9aa884" /></marker>
    <marker id="sy-k" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">The change signal — storeChanged names the store, fetch what moved</text>

  <rect class="d-panel" x="16" y="70" width="150" height="72" rx="10" />
  <text class="d-sub" x="91" y="42" text-anchor="middle" style="fill:#6b7758">on the device</text>
  <text class="d-label" x="91" y="98" text-anchor="middle">store changes</text>
  <text class="d-sub" x="91" y="116" text-anchor="middle">upload · ride</text>
  <text class="d-sub" x="91" y="132" text-anchor="middle" style="fill:#a9501c">device-side delete</text>

  <rect class="d-panel-2" x="192" y="70" width="162" height="72" rx="10" style="fill:#eef2df" />
  <text class="d-label" x="273" y="98" text-anchor="middle" style="fill:#3c6b39">storeChanged</text>
  <text class="d-sub" x="273" y="118" text-anchor="middle" style="font-size:9.5px">names the store · rev ++</text>
  <text class="d-sub" x="273" y="134" text-anchor="middle">notify (status)</text>

  <rect class="d-panel" x="380" y="70" width="150" height="72" rx="10" />
  <text class="d-sub" x="455" y="42" text-anchor="middle" style="fill:#6b7758">on the phone</text>
  <text class="d-label" x="455" y="98" text-anchor="middle">store moved →</text>
  <text class="d-sub" x="455" y="118" text-anchor="middle">download the list</text>
  <text class="d-sub" x="455" y="134" text-anchor="middle">routeList / rideList</text>

  <rect class="d-hot" x="562" y="70" width="142" height="72" rx="10" style="fill:#f8efe4" />
  <text class="d-label" x="633" y="98" text-anchor="middle" style="fill:#a9501c">fetch changed</text>
  <text class="d-sub" x="633" y="118" text-anchor="middle">objects, over</text>
  <text class="d-sub" x="633" y="134" text-anchor="middle">the CoC</text>

  <line class="d-flow" x1="166" y1="106" x2="196" y2="106" marker-end="url(#sy-a)" />
  <line class="d-flow" x1="348" y1="106" x2="378" y2="106" marker-end="url(#sy-a)" />
  <line class="d-flow" x1="530" y1="106" x2="560" y2="106" marker-end="url(#sy-a)" />

  <!-- loop back -->
  <path d="M633 142 C 633 190, 91 190, 91 144" fill="none" stroke="#9aa884" stroke-width="1.4" stroke-dasharray="5 4" marker-end="url(#sy-m)" />
  <text class="d-sub" x="360" y="202" text-anchor="middle" style="fill:#6b7758">on the next change</text>

  <!-- ackRides reverse lane -->
  <line x1="20" y1="216" x2="700" y2="216" style="stroke:#d6cda8;stroke-width:1" />
  <text class="d-tag" x="20" y="242" style="fill:#a9501c">The other direction — reconcile synced rides on connect</text>
  <rect class="d-panel" x="380" y="252" width="150" height="34" rx="9" />
  <text class="d-sub" x="455" y="273" text-anchor="middle">phone — ids it holds</text>
  <line x1="378" y1="269" x2="168" y2="269" style="stroke:#cf6a2a;stroke-width:1.6" marker-end="url(#sy-k)" />
  <text class="d-sub" x="273" y="262" text-anchor="middle" style="fill:#a9501c;font-size:9px">ackRides (GATT command)</text>
  <rect class="d-hot" x="16" y="252" width="150" height="34" rx="9" style="fill:#f8efe4" />
  <text class="d-sub" x="91" y="273" text-anchor="middle" style="fill:#a9501c">device marks synced</text>
</svg>
<figcaption><code>storeChanged</code> is the cheap "did anything change?" signal — one notification per change, naming <em>which</em> store moved so a route upload never triggers a ride re-list. On it (or on connect) the app pulls the relevant <b>list</b> object, then downloads only the objects new to it; changes arriving during a list transfer coalesce behind it. Because BLE notifications are best-effort, the app also runs a low-cadence 60-second catalog audit — a lost edge can delay a checkmark, never leave it stale until restart. A change is a change whether the phone caused it or the rider deleted something on the device: both go through the one object store and fire the same signal. The lower lane runs the other way: an <code>ackRides</code> command carries the phone's held ride ids <em>to</em> the device, which marks them synced (below).</figcaption>
</figure>

The device is the other half of this loop. A change doesn't only come *from* the
phone — the rider can delete a stored route from the device's Route overview or a
tracked ride from its Ride detail, each with the same guarded hold-to-delete row
(see the
[UI system](../ui/#deleting-things-the-hold-to-delete-footer)). A device-side
delete goes **through the same object store** the wire commits do, so it bumps
the `revision`, fires `storeChanged`, and shows up to the phone as *"the ride
store moved"* on the next notify — no separate "the device deleted something"
message, and no way for the two to disagree about what's on the card. The phone
reconciles by re-reading the list and tombstoning what vanished, exactly as it
would after any other change.

**Trips ride the same loop.** A trip (`type 9`) is a third store alongside
routes and rides — tiny metadata that references member routes by object id — and
its catalog (`tripList`, `type 10`) reconciles exactly like `routeList`: the
device fires `storeChanged` for the trip store, the phone pulls the list, and each
entry's `crc32` is the fingerprint that decides whether a stored trip is current.
A whole-trip upload is *stages first, trip object last* — every member route
commits, then the trip that references them — so an interrupted push never leaves
a trip pointing at a route that isn't there, and re-running skips whatever already
landed. A device-side "delete this whole folder" is a cascade the device composes
from ordinary object deletes (the member routes, then the trip), each flowing back
as its own `storeChanged`; the wire trip delete itself is non-cascading. The
byte layout is the [BLE interface spec §7.4 / §7.7](src:specs/obc-ble-interface-spec.md).

**Ids are never reused — which is what keeps the bookkeeping honest.** The phone
persists *"I uploaded route 7"* and *"I've synced ride 12"* by durable object id.
If a delete freed id 7 and the next upload re-took it, the phone's note would
now point at a *different* route. So the device mints strictly above a persistent
**floor** — an SD filename guards a stored id, an RRAM floor guards a *deleted*
id — and a freed id stays retired. That invariant is what a trustworthy
`storeChanged` rests on — *as long as the id space itself never resets underneath
it.* When it does — a chip wipe, a factory reset, a freshly-formatted card — a
**store epoch** makes the reset visible, so the phone never mistakes a reborn id
for the old object (next section).

### Synced rides — reconciled state, not event inference

A tracked ride is precious (unlike a route, the phone can't re-upload it), so the
device keeps a **"synced" flag** per ride — does a durable copy of this one exist
somewhere off the device? It drives the small check mark on a synced Rides-list
row (an unsynced ride shows nothing there) and the *"synced" / "not synced"* slot
in the Ride detail's title bar, so a rider deleting an un-downloaded ride is told
what they're about to lose.

The naïve way to set that flag is to flip it when a ride download completes. But
that makes it an *event inference* — and events are lossy. A ride synced before
the device tracked the flag, a card reflashed, an app reinstalled: any of these
leaves the device's flag out of step with what the phone actually holds, and
*permanently*, because a ride the phone already has is never re-downloaded to
correct it. So the flag is instead **reconciled state**. The phone's library is
the ground truth for "I have this ride", and on every connect it sends the device
the list of ride ids it holds — a small `ackRides` command. The device sets
(never clears) the synced flag for each; a change bumps the ride revision so the
Rides screen's cue updates live. The flag becomes *"a peer has confirmed it holds
this"*, self-healing on every reconnect rather than riding on a single download
event landing.

**Who is allowed to say it.** Read the flag by what it *does* — guard a delete,
colour a cue, and anchor the auto-expiry countdown
([#638](https://github.com/timohueser/OpenBikeComputer/issues/638), whose
`synced_at` stamp is written beside it) — and it is a **durability predicate**,
not a statement about iPhones. Which makes "who may set it" a real question,
because the answer decides whether a ride can be auto-deleted while it exists
nowhere else. Three peers can pull a ride; only two of them may say anything
about it:

| Peer | Acks? | Why |
| :-- | :-- | :-- |
| The phone, over BLE | yes — and re-sends its whole set on every connect | it keeps a library, so it can heal the flag as well as set it |
| The desktop app, over USB | yes, **after `fsync`** | it writes into a folder the rider chose and can back up |
| The hosted site, over WebUSB | **never** | a browser download is a file the rider may cancel at the save dialog, and the site keeps no record of what it handed over |

The desktop app acking is what keeps auto-expiry alive for a rider with no
iPhone; it costs no protocol change, because `ackRides` lives in the object store
rather than the BLE plane and is monotonic, so a phone's heal and a desktop ack
merge in either order. The browser deliberately gives that up: its ride export
([#904](https://github.com/timohueser/OpenBikeComputer/issues/904)) is a pure
read that leaves the flag, the sidecar and the countdown exactly as it found
them, and says so on screen — an export is not a backup.

The invariant underneath all three is **ack after the copy is durable, never on
transfer completion**. Acking when the last byte arrives starts a countdown
against a ride that is not yet on anyone's disk, which is the single way this
feature can lose data.

"Durable" is a syscall, not a hope. On the desktop side a pulled ride is written
to a temporary sibling, `fsync`ed, renamed over its destination, and the
*directory* is `fsync`ed too — that last step is what makes the rename survive a
power cut, and skipping it is the classic failure where the bytes are on the disk
and the name pointing at them is not. Only then does the index that lists the ride
commit, by the same four steps; only then does the ack go out. A crash anywhere in
the middle leaves an index that does not mention the ride, so the next pull
fetches it again and the device was never told anything — the direction that costs
a re-download rather than a ride. And because the ack list is computed by asking
the *filesystem* which rides are there, rather than by remembering which ones were
just written, a file the rider deleted in Finder drops out of it on its own.

Two more rules fall out of the same reasoning, and both are about *fetching*
rather than acking. The peer always pulls the **whole** ride list and dedupes
against its own library: the device's `synced` flag is a statement about
durability somewhere else, so using it to decide what to download would skip
exactly the rides a second peer has never seen. And a ride is keyed by
`(serial, epoch, id)` — the [id era](#store-epochs-which-id-era-youre-talking-to)
below — never by the bare id, because a bare-id library silently discards a new
ride that reused a recycled one.

### The trusted clock and route retention

Two more things ride every connect, both from the storage auto-expiry work
([#638](https://github.com/timohueser/OpenBikeComputer/issues/638)). First:
immediately after encryption and **before the first `ackRides`**, the phone sends
a **`setClock`** command — the current UTC plus the phone's local offset. The
device has no battery-backed clock, so this (or a GPS fix) is what establishes a
*trusted* wall clock for the boot, the safety gate the device's [auto-delete
sweep](../ui/#the-device-has-no-clock-so-deletion-waits-for-a-trusted-one) won't
act without. The ordering is deliberate: because `setClock` lands first, the
`ackRides` that flags a ride synced runs under a trusted clock, so the moment it
stamps as that ride's *synced-at* — the anchor for the ride's eventual
auto-delete — is a real timestamp, not a stale set-point.

Second: a route's **retention** — its "delete after this long unused" window — is
set from the app with a **`setRouteRetention`** command (object id + level), *not*
by re-uploading the route. That split is the interesting part. The [OBCR route
file](../formats/) is **byte-pinned** — an upload's payload is exactly the route's
bytes, and stays that way — but retention is mutable device-local state that
changes without the geometry changing. So it never enters the file: it lives in an
SD sidecar (route id → level + last-used), travels as a command, and the device
reports each route's computed `expires_at` back in its `routeList` entry — which
grew a small tail to carry it. Formats stay pinned; the mutable state routes
*around* them. The command layouts, the connect-ordering rules, and the 84-byte
list entry are the [BLE interface spec §4.4 / §7.4](src:specs/obc-ble-interface-spec.md).

**A whole trip is one retention choice.** When you upload a *trip*, the confirm
sheet shows a single **Auto-delete** picker, and that one choice is the
postcondition for **every** member route — a trip is one unit, so the trip-level
pick overrides whatever level a member route carried on its own. The subtlety is
that a whole-trip upload skips the *bytes* of any stage the device already holds
(same content, nothing to re-send) — but a skipped stage is skipped only for
transfer, **never** for policy: the retention command still lands on it, so the
one trip choice reaches the already-current stages exactly as it reaches the
freshly-uploaded ones. A re-run at the level a stage already holds sends nothing
(idempotent), and an old device with no expiry support shows no picker and
receives no retention command at all.

## Store epochs — which id era you're talking to

Everything above trusts durable ids to keep meaning the same object next connect.
Within a store that holds — but an id space can *reset*. A full-chip reflash, a
factory reset, a torn settings write, a freshly-formatted card: any of these can
lose the floor that guards deleted ids, and the device's next upload re-mints ids
that months-old phone-side state still points at. Reuse id 7, and the phone's
*"route 7"* silently **aliases a different route** — a green *"up to date"* badge
for the wrong thing, or worse, an upload that replace-by-ids over the *wrong* route
on the device. (This bit the bench on 2026-07-12: new rides filtered as *"already
synced"* while the app insisted everything was up to date.)

The fix names each id era with something the phone can watch change: a **store
epoch** — a `u32` random nonce. The device serves it in the same **open,
pre-pairing** `protocolVersion` read the app already performs first on every
connect, widened from a bare version to `version u16 · store_epoch u32 ·
obcm_version u8 · feature_bits u32`. So before `ackRides` or any reconcile write
fires, the app knows the protocol version, which era it is looking at, which map
format the device reads, and which optional contracts it speaks (below).

**The epoch lives on the card.** It is persisted as a tiny **`EPOCH.OBE`** file in
the card root — the record layout and its torn-file → fresh-era conventions are in
the [BLE interface spec §1](src:specs/obc-ble-interface-spec.md) — so the store carries
its *own* era name. Swap the card and you transplant the
store: the epoch travels with it, so the same device never conflates two cards' id
spaces — and a card written by a *different* device presents *its own* epoch, a
distinct era on this device by construction. A lost RRAM floor still stamps a
**fresh** epoch onto the card even when the card's epoch file survived intact: a
compromised id namespace is a new era regardless of where the name is stored.

**The app scopes every id-keyed fact by `(device serial, store epoch)`.** Ride
entries, the synced set, delete tombstones, route links — all keyed by that triple,
valid only when all three match the connected device. An era change then needs
**no migration code**: the old era's keys simply never match again — they go
archival *by construction* — and the new era starts empty. There is no multi-step
re-key for an app kill to tear halfway.

**No store ⇒ no epoch ⇒ nothing stamped.** A device with no card mounted has
nothing to name and nothing to prove, so its `protocolVersion` read degrades to the
**2-byte, version-only** form. The app reads the missing epoch as a **failed
identity read** — not as epoch `0`, which is a legal value — and **fails closed**:
no `ackRides`, no reconcile writes, no badges (plain library browsing is
unaffected). The same gate catches a read that genuinely failed, so a device whose
era can't be established can never stamp a checkmark under an unknown id space.

The widened read's bytes, the exact mint rule, and the full list of era events live
in the [BLE interface spec §1](src:specs/obc-ble-interface-spec.md); the design rationale
is epic [#632](https://github.com/timohueser/OpenBikeComputer/issues/632) item 5,
with the card-resident decision in
[#776](https://github.com/timohueser/OpenBikeComputer/issues/776).

**Two more fields, and why neither is a version bump.** The read carries a third value:
`obcm_version`, the [OBCM map-format version](../formats/#the-catalog-the-map-builders-source-of-truth)
this firmware's reader reads. It is a *different number in a different sequence* from
the protocol version beside it — one is this wire contract, the other is the file
format on the card — and neither can be derived from the other, nor from the firmware
revision string, which maps to a format version only through a table nobody keeps. A
catalog builder needs it before offering assembled map bytes, and had nothing to read it from.

And a fourth: `feature_bits`, a `u32` of **optional contracts this build actually
implements**. The first bit is Weather ([§11](src:specs/obc-ble-interface-spec.md)) — the
request service, the request context, the weather object type and the refresh setting, one
bit for all four because a phone that can read a weather request but cannot upload the
answer has nothing to offer. It is deliberately not inferred from the firmware revision:
that string maps to a feature set through the same table nobody keeps. A device announces
what is *there*, so a build carrying the layouts but not yet raising requests announces
nothing — a phone that saw the bit set would sit waiting for an advertisement that never comes.

Appending it changed the read's length, and a read whose length changed sounds like a
protocol break. It isn't, because **the length has always been the version mechanism
here**: a device with no card already served a short read, so the decode was never "expect
exactly *n* bytes" — it is "take each field if that many bytes arrived, ignore anything
past the ones you know". Four lengths now exist: **11** with a mounted store, **7** from a
firmware predating the capability word, **6** from one predating the map version too, and
**2** with no card. A field that didn't arrive reads as *unknown*, never as a fabricated `0`
— `0` is a legal store epoch, OBCM `0` would read as "supports map format v0" and refuse
every real map, and a fabricated capability word of `0` would make a diagnostic lie about
which firmware generation answered. (Both *absent* and a genuine `0` mean no weather, so the
behaviour is the same; it is the provenance that would be lost.) A **partial** capability
word — 8, 9 or 10 bytes — reads as absent rather than as the bytes that turned up: three
bytes of a `u32` are a broken read, not a small feature set, and treating them as data could
claim a contract the device never announced. So an old app against a new device
loses nothing it had, and a new site against an old device gets *unknown* and falls back to
offering the download with its version stated. Bumping the protocol version would instead
stop both — a hard mutual "we can't talk" for a field that is allowed to be missing.

## Verified badges — presence you can prove

A route the phone uploaded earns an **"on device"** badge in the app. A badge is
only honest, though, if the app can *prove* the device still holds *that* route —
not merely a route wearing the same id. v2 makes the proof cheap: each `routeList`
entry carries the **whole-object CRC-32** the device computed at upload commit
(persisted in a small `/routes` sidecar; `0` means *not yet known*, filled lazily
the first time a side-loaded file is listed).

**Proof-only presence.** The badge lights only when the per-serial link is valid
**and** the catalog CRC matches the CRC the app recorded at upload — never on a
matching id alone. Combined with epoch scoping, a stale link that outlived an era
change simply fails to match and shows nothing. **No checkmark without proof.**

**Adopt by content.** The CRC also heals the reverse case. After an app reinstall
(or on a second phone) the device still holds routes the app has no link to — but
an *unlinked* catalog entry whose CRC matches a route the app holds is **adopted**:
the badge lights with no upload, and a later upload of that route **replaces by id**
rather than creating a duplicate. Anything the app can't prove shows no badge — the
worst case is a needless re-upload, which adoption makes rare.

**The list never lies "up to date".** Past the device's catalog cap the store scan
drops the excess in FAT order — in v1, silently. The v2 list header carries a
`total` alongside the `count` actually returned: `total > count` means the object
was truncated, and the app surfaces a one-line *"some items couldn't be listed"*
warning instead of quietly reporting everything is synced.

The `routeList` entry's `crc32`, the 6-byte list header with `total`, and their
exact byte layout are the [BLE interface spec §7.4](src:specs/obc-ble-interface-spec.md);
the proof-only badge and adopt-by-content behaviour are epic
[#632](https://github.com/timohueser/OpenBikeComputer/issues/632) item 6.

## Pairing, and staying paired

Access is gated by a **bond** — a one-time, mutually-authenticated pairing. The
device is a *display-only* peer: it shows a **6-digit passkey** on its screen
that the rider types into the phone's system dialog. That's LE Secure
Connections passkey entry — man-in-the-middle-protected — and the on-screen code
is what makes it safe: **physical possession of the device is the control.** On
the device that code is a full-screen [**passkey card**](../ui/#the-passkey-card):
the host pushes it the instant the radio raises a passkey and pops it the instant
pairing ends, and it's deliberately non-dismissible — no button can lose the code
mid-pairing — because the SMP handshake time-boxes the window anyway.

There is exactly one bonded peer — and while that bond exists the device
**rejects any new pairing attempt**: a stranger's phone gets a generic pairing
failure and the device screen shows nothing (no passkey card, because there's no
pairing to complete). Re-pairing (a new or reset phone) goes through the
hold-guarded **Forget phone** action in the device's Settings ▸ Bluetooth, which
clears the bond and re-opens pairing — so physical possession still gates the
swap, at the *clear* step. This *reverses* an earlier "a fresh pairing replaces
the stored bond" rule: a lost or wiped phone can no longer silently re-pair.
There is one more way to clear the bond, and it needs no on-device step: the
**bonded** phone can send a `forgetBond` command over its own encrypted link, so
the app's "Forget device" dissolves the device's side of the bond too rather than
leaving the pair wedged (a one-sided app forget would otherwise keep hitting the
reject). It's safe precisely because it rides the bonded link — only the paired
phone can issue it, a stranger never can. The same screen carries the Bluetooth
**off** switch: off stops advertising and drops the link, while the bond survives
for when the radio comes back.

<figure class="fig">
<svg viewBox="0 0 720 400" role="img" aria-label="Pairing, reconnect, and rejection in three rows. Top row, first pairing, done once: the device shows a six-digit passkey on its screen; the rider reads it and types it into the phone; the two run an LESC elliptic-curve key exchange; both sides store the resulting bond keys. Middle row, every time after, silent: the device advertises with a stable address; the phone recognises that identity from the bond; the two re-encrypt with the stored long-term key and the phone's rotating address is resolved via the stored identity key; the result is a connected, encrypted link with no dialog. Bottom row, reject-when-bonded: a different phone tries to pair while a bond already exists; the device suppresses its passkey and drops the link; the other phone sees only a generic pairing failure; the only way through is the rider running Forget phone on the device to clear the bond.">
  <defs>
    <marker id="pk-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>

  <!-- Row 1: first pairing -->
  <text class="d-tag" x="20" y="24">① First pairing — once</text>

  <rect class="d-panel" x="16" y="38" width="150" height="60" rx="10" />
  <text class="d-sub" x="91" y="60" text-anchor="middle">device shows</text>
  <text class="d-title" x="91" y="82" text-anchor="middle" style="fill:#a9501c">428 913</text>

  <rect class="d-panel-2" x="222" y="38" width="150" height="60" rx="10" />
  <text class="d-sub" x="297" y="64" text-anchor="middle">rider reads it,</text>
  <text class="d-sub" x="297" y="80" text-anchor="middle">types it on the phone</text>

  <rect class="d-panel-2" x="428" y="38" width="150" height="60" rx="10" style="fill:#eef2df" />
  <text class="d-label" x="503" y="62" text-anchor="middle" style="fill:#3c6b39">LESC ECDH</text>
  <text class="d-sub" x="503" y="80" text-anchor="middle">MITM-protected</text>

  <rect class="d-hot" x="622" y="38" width="82" height="60" rx="10" style="fill:#f8efe4" />
  <text class="d-sub" x="663" y="60" text-anchor="middle">bond</text>
  <text class="d-sub" x="663" y="78" text-anchor="middle">stored</text>

  <line class="d-flow" x1="166" y1="68" x2="220" y2="68" marker-end="url(#pk-a)" />
  <line class="d-flow" x1="372" y1="68" x2="426" y2="68" marker-end="url(#pk-a)" />
  <line class="d-flow" x1="578" y1="68" x2="620" y2="68" marker-end="url(#pk-a)" />

  <!-- divider -->
  <line x1="20" y1="130" x2="700" y2="130" style="stroke:#d6cda8;stroke-width:1" />

  <!-- Row 2: reconnect -->
  <text class="d-tag" x="20" y="164">② Every time after — silent</text>

  <rect class="d-panel" x="16" y="180" width="150" height="66" rx="10" />
  <text class="d-sub" x="91" y="206" text-anchor="middle">device advertises</text>
  <text class="d-sub" x="91" y="224" text-anchor="middle" style="fill:#3c6b39">stable address</text>

  <rect class="d-panel-2" x="222" y="180" width="150" height="66" rx="10" />
  <text class="d-sub" x="297" y="206" text-anchor="middle">phone knows</text>
  <text class="d-sub" x="297" y="224" text-anchor="middle">this identity</text>

  <rect class="d-panel-2" x="424" y="180" width="158" height="66" rx="10" style="fill:#eef2df" />
  <text class="d-sub" x="503" y="202" text-anchor="middle" style="font-size:9.5px">re-encrypt · stored LTK</text>
  <text class="d-sub" x="503" y="220" text-anchor="middle" style="font-size:9.5px">resolve RPA · stored IRK</text>

  <rect class="d-hot" x="622" y="180" width="82" height="66" rx="10" style="fill:#f8efe4" />
  <text class="d-sub" x="663" y="204" text-anchor="middle">connected</text>
  <text class="d-sub" x="663" y="222" text-anchor="middle" style="fill:#a9501c">encrypted</text>
  <text class="d-sub" x="663" y="238" text-anchor="middle">no dialog</text>

  <line class="d-flow" x1="166" y1="213" x2="220" y2="213" marker-end="url(#pk-a)" />
  <line class="d-flow" x1="372" y1="213" x2="426" y2="213" marker-end="url(#pk-a)" />
  <line class="d-flow" x1="578" y1="213" x2="620" y2="213" marker-end="url(#pk-a)" />
  <text class="d-sub" x="360" y="266" text-anchor="middle" style="fill:#6b7758">bonded + powered + in range  ⇒  connected + encrypted, no interaction</text>

  <!-- divider -->
  <line x1="20" y1="290" x2="700" y2="290" style="stroke:#d6cda8;stroke-width:1" />

  <!-- Row 3: reject-when-bonded -->
  <text class="d-tag" x="20" y="322" style="fill:#a9501c">③ Another phone, while bonded — rejected</text>

  <rect class="d-panel" x="16" y="336" width="150" height="52" rx="10" />
  <text class="d-sub" x="91" y="358" text-anchor="middle">a different phone</text>
  <text class="d-sub" x="91" y="374" text-anchor="middle">tries to pair</text>

  <rect class="d-panel-2" x="222" y="336" width="184" height="52" rx="10" style="fill:#f4e7de" />
  <text class="d-sub" x="314" y="358" text-anchor="middle" style="fill:#a9501c">bond exists →</text>
  <text class="d-sub" x="314" y="374" text-anchor="middle" style="fill:#a9501c">no passkey, link dropped</text>

  <rect class="d-panel-2" x="462" y="336" width="120" height="52" rx="10" />
  <text class="d-sub" x="522" y="358" text-anchor="middle">phone sees a</text>
  <text class="d-sub" x="522" y="374" text-anchor="middle">generic failure</text>

  <rect class="d-hot" x="606" y="336" width="98" height="52" rx="10" style="fill:#f8efe4" />
  <text class="d-sub" x="655" y="356" text-anchor="middle" style="fill:#a9501c">only way in:</text>
  <text class="d-sub" x="655" y="372" text-anchor="middle">Forget phone</text>

  <line class="d-flow" x1="166" y1="362" x2="220" y2="362" marker-end="url(#pk-a)" />
  <line class="d-flow" x1="406" y1="362" x2="460" y2="362" marker-end="url(#pk-a)" />
</svg>
<figcaption>Pairing happens once, with the passkey on the glass. After that the device keeps a <b>stable</b> address (no device-side privacy rotation), so the phone — which stored that identity at bonding — reconnects silently on any contact and re-encrypts with the stored long-term key; the phone's own rotating address is resolved back to it via the stored identity key. The bond lives in the device's persistent settings storage, so it survives power cycles <em>and firmware updates</em>. The third row is the guard: while a bond exists, <b>a second phone can't pair</b> — the device shows no passkey and drops the attempt, so the interloper sees only a generic failure and the rider must deliberately <b>Forget phone</b> to open a swap.</figcaption>
</figure>

What the bond protects, and what stays open:

| Surface | Before pairing |
|---------|----------------|
| Device Information · Battery · `protocolVersion` (version **+ store epoch**) | **open** — so the app reads identity, version, *and* the store epoch *before* pairing |
| every other OBC Control characteristic | **denied** — needs the encrypted, authenticated link |
| the L2CAP CoC | **denied** — opening it on an unencrypted link is refused |

Leaving identity and the protocol version readable pre-bond is deliberate: the
app checks it's talking to a compatible device (and surfaces a mismatch as a
banner rather than trapping) before it ever asks to pair. The same open read now
also carries the [store epoch](#store-epochs-which-id-era-youre-talking-to), so it
lands *before* any `ackRides` — the app always knows the id era before it stamps
anything, which is exactly what the fail-closed gate needs.

## Sensors — the device as BLE central

The phone link is only half of the device's Bluetooth life. To a *phone* the
device is a **peripheral** — the phone scans, connects, and drives it. To a
**heart-rate strap, power meter, or cadence sensor** it is the opposite: the
**central**, the side that scans, connects, and subscribes. Both roles run on
the **one radio**. trouble-host 0.7 runs a peripheral and a central role
concurrently on a single `Stack`, and MPSL time-slices the airtime between them —
so a sensor link and the phone link coexist with no second radio and no mode
switch. The whole feature is **BLE-only**; there is no ANT+.

<figure class="fig">
<svg viewBox="0 0 720 340" role="img" aria-label="The device plays two BLE roles on one radio. On the left the companion phone is the central and the device is the peripheral it connects to — the phone link. On the right the device is itself the central, connecting out to three sensors: a heart-rate strap, a power meter, and a cadence sensor. A band along the bottom notes that a single radio carries both directions, with MPSL time-slicing the airtime between the peripheral (phone) and central (sensor) roles, and that sensors are open GATT servers connected by stored address with no bond, one saved slot per quantity.">
  <defs>
    <marker id="se-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="se-c" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#33575b" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Two roles, one radio — peripheral to the phone, central to the sensors</text>

  <!-- phone (device = peripheral) -->
  <rect class="d-panel" x="24" y="128" width="128" height="84" rx="11" />
  <text class="d-title" x="88" y="162" text-anchor="middle">companion app</text>
  <text class="d-sub" x="88" y="182" text-anchor="middle">BLE central</text>
  <text class="d-sub" x="88" y="198" text-anchor="middle">(iPhone)</text>

  <!-- device -->
  <rect class="d-panel" x="278" y="112" width="184" height="116" rx="12" style="fill:#eef2df" />
  <text class="d-title" x="370" y="150" text-anchor="middle">OBC device</text>
  <text class="d-sub" x="370" y="172" text-anchor="middle" style="fill:#3c6b39">peripheral · to phone</text>
  <text class="d-sub" x="370" y="190" text-anchor="middle" style="fill:#33575b">central · to sensors</text>
  <text class="d-sub" x="370" y="210" text-anchor="middle">one radio · nRF54L</text>

  <!-- phone <-> device -->
  <line class="d-flow" x1="152" y1="170" x2="276" y2="170" marker-start="url(#se-a)" marker-end="url(#se-a)" />
  <text class="d-sub" x="214" y="160" text-anchor="middle" style="font-size:9px;fill:#3c6b39">the phone link</text>

  <!-- sensors (device = central) -->
  <rect class="d-panel-2" x="566" y="70" width="138" height="48" rx="10" />
  <text class="d-label" x="635" y="90" text-anchor="middle" style="font-size:10.5px">heart-rate strap</text>
  <text class="d-sub" x="635" y="106" text-anchor="middle" style="font-size:8.5px">HRS · 0x180D</text>

  <rect class="d-panel-2" x="566" y="146" width="138" height="48" rx="10" />
  <text class="d-label" x="635" y="166" text-anchor="middle" style="font-size:10.5px">power meter</text>
  <text class="d-sub" x="635" y="182" text-anchor="middle" style="font-size:8.5px">Cycling Power · 0x1818</text>

  <rect class="d-panel-2" x="566" y="222" width="138" height="48" rx="10" />
  <text class="d-label" x="635" y="242" text-anchor="middle" style="font-size:10.5px">cadence sensor</text>
  <text class="d-sub" x="635" y="258" text-anchor="middle" style="font-size:8.5px">CSC · 0x1816</text>

  <!-- device -> each sensor -->
  <line class="d-flow" x1="462" y1="150" x2="564" y2="96" style="stroke:#33575b" marker-end="url(#se-c)" />
  <line class="d-flow" x1="462" y1="170" x2="564" y2="170" style="stroke:#33575b" marker-end="url(#se-c)" />
  <line class="d-flow" x1="462" y1="192" x2="564" y2="244" style="stroke:#33575b" marker-end="url(#se-c)" />
  <text class="d-sub" x="524" y="160" text-anchor="middle" style="font-size:9px;fill:#33575b">scan · connect · subscribe</text>

  <!-- bottom band -->
  <rect class="d-panel-2" x="24" y="292" width="680" height="40" rx="9" />
  <text class="d-sub" x="364" y="309" text-anchor="middle" style="font-size:9.5px">one radio — <tspan style="fill:#a9501c">MPSL time-slices</tspan> the peripheral (phone) and central (sensor) roles; no second radio</text>
  <text class="d-sub" x="364" y="325" text-anchor="middle" style="font-size:9px">sensors are open GATT servers — connected by stored address, <tspan style="fill:#a9501c">no bond</tspan>, one saved slot per quantity</text>
</svg>
<figcaption>The device wears both BLE hats at once. To the phone it is the <b>peripheral</b> (the phone link, left); to each sensor it is the <b>central</b> that scans, connects, and subscribes (right). A single radio carries both — <b>MPSL time-slices</b> the airtime, so there is no second radio and no switching between roles. Sensors need no bond: they are open GATT servers the manager reaches by a <b>stored address</b>, one saved slot per quantity (heart rate · power · cadence).</figcaption>
</figure>

**The manager loop: scan → connect → subscribe → decode → dispatch.** A small
central-role task runs beside the peripheral lifecycle. Given the radio on and a
sensor saved, it connects to the stored address, discovers the profile's
measurement characteristic, reads the battery level once, subscribes to the
notifications, and then just pumps them: each notification is decoded and its raw
value dispatched. The decode is pure `no_std` byte→value parsing that lives in the
radio-free [`obc-ble`](src:firmware/obc-ble) crate — Heart Rate Measurement
(`0x2A37`), Cycling Power Measurement (`0x2A63`), CSC Measurement (`0x2A5B`), and
Battery Level (`0x2A19`), plus a crank-revolution→rpm accumulator — so it is
host-tested with no radio in the loop. The dispatched value lands in an
[`obc-platform`](src:firmware/obc-platform/src/sensor_hub.rs) mailbox the app
drains like any other sensor — the **same** mailbox the simulator's sliders and
the USB-injection `H`/`P`/`R` lines feed, so the app can't tell a real strap from
an injected one (last-writer-wins). The app never learns BLE exists; to it a
sensor is just *a thing that produces bpm.*

**No bonding — sensors are open.** A strap or power meter is an open GATT server:
no pairing, no passkey, no encryption. The manager connects by the address the
rider saved and that's it. The phone-bond machinery is completely untouched — the
single bond slot is the phone's alone, and a sensor never consumes it. (Sensor
bonding, encrypted sensors, and ANT+ are all deliberately out of scope.)

**One slot per quantity.** There are three fixed slots — heart rate, power,
cadence — one saved sensor each. Cadence is the one *arbitrated* quantity: a saved
**dedicated** cadence sensor owns it, but with none saved the crank data a power
meter already reports fills the cadence slot, so a power meter doubles as a
cadence source for free.

**What a link costs, and the cap.** Every central link the host tracks costs real
controller memory — about 2.3 KB of SoftDevice-Controller buffers plus host arena —
so the number of concurrent sensor links is a pinned constant, `SENSOR_LINKS`,
arbitrated by the same compile-time RAM budget assert as the rest of the BLE
statics (the [#677](https://github.com/timohueser/OpenBikeComputer/issues/677)
rule: everything sizeable is a summed `.bss` static). On the 256 KB DK it is **1** —
the phone plus one sensor, enough to bring up a Garmin watch broadcasting HR; the
512 KB **LM20 raises it to 3**, so all three quantities can be live at once. One
link is about **+7 KB** of RAM over the phone-only build; three is about +12 KB.
Runtime behaviour matches the phone link's discipline: the manager auto-reconnects
with a ~15 s backoff whenever the radio is on and a slot is saved, a sensor link
**parks with the radio switch** exactly like the phone link, and a value older than
**5 s** renders `--` and records as *absent* — a dropped strap must never freeze
its last reading into the log.

**Where the values go.** Live, they drive three [stat tiles](../ui/#the-sensors-screen) —
heart rate, power, cadence — plus per-ride averages and maxima. Recorded, they
widen the ride's on-disk records: the freshest sample is stamped onto each logged
track point and, at Finish, carried into the **ride object v2** — the very object
the phone downloads. There is deliberately **no live sensor streaming to the
phone**; like everything else, the phone gets the numbers *after* the ride, inside
the ride object it syncs. Those recording formats — the track log and the v1/v2
ride object — are the [recorded-rides section](../formats/#recorded-rides-the-track-log-and-the-ride-object)
of the data-formats page (normative bytes in the
[BLE interface spec §7.2](src:specs/obc-ble-interface-spec.md)).

---

## The same link, down a cable

The nRF54LM20 has a real USB device peripheral, and pointing it at this protocol
turns out to cost almost nothing — because of a decision made long before USB was
on the table.

Look again at what a transfer actually needs from its transport. The bulk plane
needs a channel that is **reliable, ordered, and unframed**; that is exactly what
principle two asks of the CoC, and exactly what a **USB bulk endpoint** is. So
the object stream — descriptors, payload bytes, terminal result — crosses a
cable with *no translation whatsoever*. The descriptor still carries the same
whole-object CRC, but verification is policy rather than framing: routes, trips,
firmware and BLE traffic keep the end-to-end pass; USB map files avoid doing the
same serial work twice and lean on USB packet CRC/retry, the card's block CRC/ECC,
the exact announced length, format validation and the magic-last commit. Same
state machine, same fixtures, same bytes on the wire.

Only the control plane needs anything at all, and it needs one byte. GATT gives
each control message its own addressed characteristic, so "which message is
this?" is answered by the transport rather than by any byte of ours. USB has one
endpoint pair, so that routing becomes a leading **selector byte** — and the rest
of the frame is the *exact* payload the matching characteristic would have
carried. That is the whole delta. USB is a second **transport**, not a second
protocol.

Five consequences are worth naming, because they are what makes the wired path a
real product rather than a demo:

- **The cable is what brings the plane into existence.** A bike computer spends
  almost all of its life with nothing plugged into it, so the wired plane is
  built *when a cable appears* and parked again when it goes — not armed at boot
  and left waiting. That ordering is not tidiness: the USB core is unpowered
  until the hardware reports bus voltage, and a device that reached into it
  anyway would trade the common case (riding) for the rare one (a transfer). The
  device says which state it is in on every boot, because "no cable" and "USB
  broken" must never look the same from the outside. And the *waiting* is free:
  the parked plane is asleep on the bus-voltage interrupt rather than asking a
  timer how things are, so a bike ridden all day with nothing plugged in spends
  no energy at all on the possibility of a cable. On a device running off a
  battery, "poll for the rare case" is a cost paid by every ride.
- **The device keeps working while a map lands.** Storage is arbitrated behind
  one async mutex, and each store call takes it only for its own duration — so
  the ride loop's redraws interleave between chunks. This is why USB **Mass
  Storage** was rejected outright: handing a host raw block access would force
  the firmware to release the card entirely (two filesystem writers is
  corruption), and you would be back to a device that becomes a disk instead of
  remaining a bike computer.
- **A broken map cannot lock out its replacement.** If no map mounts at boot, the
  fault screen stays visible but the device still brings up the USB plane and
  grants it the upload arena. The builder can replace the unreadable map at full
  speed; a successful reboot then returns to the normal ride application.
- **One transfer at a time means one across *both* wires.** The gate is shared
  rather than per-transport, because what it protects is the device's single
  upload temp and open download source — not the wire. Start a transfer over the
  cable while the phone is mid-upload and you get the same typed `busy` the phone
  would get from another phone.
- **A cancel has to reach the wire, not just the caller.** A browser cannot
  cancel a USB transfer it has already submitted, so it releases the *caller* and
  lets the transfer settle into nothing — which is survivable only because an
  interrupted exchange is always followed by a channel reset. A native host can
  cancel for real, and must: after an abort the device stops sending by design,
  so a read left waiting on it would hold the endpoint forever and the link would
  be dead while still looking alive.

The map builder uses one small USB-only read beside those shared objects: the
device reports the mounted card's free-byte count from FAT32's cached FSInfo
sector. Step 4 compares that number with the selected assembly plus its safety
allowance before enabling Send; no guessed card capacity and no stale desktop
setting stands in for the card that is actually connected.

A cell-built map then crosses as what it is: one object — one announce, one
progress line, one commit, whatever the map weighs. What guarantees it arrived
is the **whole-object CRC-32** the descriptor announces and the device checks
before it commits; the page has nothing of its own to check a map against, which
is the same position a rider's hand-picked `.obcm` is in.

There is no assemble-and-send-in-one-motion path today: the builder saves the
assembled file, and sending it is the ordinary file upload. The direct path is a
real design — the map never touching the disk between the assembler and the card
— and it returns with the board cutover, as a single-object `PUT` under protocol
major 4 rather than as anything map-shaped.

Everything else about the link is unchanged by the choice of wire: the same
objects, the same restart-don't-resume rule, the same change signal, the same
`status` ordering domain. Pairing is the one genuine exception — encryption and
bonding are BLE mechanisms, and the cable's authentication is that someone is
holding it.

## Where this lives

- The wire contract, normative: [`obc-ble-interface-spec.md`](src:specs/obc-ble-interface-spec.md) (§10 is the USB binding)
- The host-tested, radio-free core — descriptor codecs, CRC-32, the transfer state machine: [`obc-ble`](src:firmware/obc-ble) ([`transfer.rs`](src:firmware/obc-ble/src/transfer.rs) · [`descriptor.rs`](src:firmware/obc-ble/src/descriptor.rs))
- On the device, shared by every transport — the command handler, descriptor classification, the identity blobs, the one object store, and the cross-transport transfer gate: [`obc-fw-nrf54l/src/link/`](src:firmware/obc-fw-nrf54l/src/link)
- On the device — the GATT server, connection lifecycle, and the CoC data plane: [`obc-fw-nrf54l/src/ble/`](src:firmware/obc-fw-nrf54l/src/ble)
- On the device — the USB vendor interface, the selector envelope, and the bulk object stream: [`obc-fw-nrf54l/src/usb/`](src:firmware/obc-fw-nrf54l/src/usb)
- The central-role **sensor manager** — scan / connect / subscribe / decode / dispatch, the `SENSOR_LINKS` cap and its budget: [`obc-fw-nrf54l/src/ble/sensors.rs`](src:firmware/obc-fw-nrf54l/src/ble/sensors.rs)
- The radio-free sensor profile codecs, the advertisement classifier, and the crank→rpm accumulator: [`obc-ble`](src:firmware/obc-ble) (`sensors.rs`)
- The app-facing sensor mailboxes both the radio manager and the injection path feed — one instance-owned `SensorHub`, handed to each task at spawn: [`obc-platform/src/sensor_hub.rs`](src:firmware/obc-platform/src/sensor_hub.rs)
- The device UI's link seam — the connected indicator, passkey card, and upload prompts consume this: [`obc-app/src/ble.rs`](src:firmware/obc-app/src/ble.rs) (and the [UI system](../ui/#screens-the-companion-link-pushes))
- The phone side — the SwiftUI companion app and its transport layer: [`companion-ios/`](src:companion-ios)
- The host side — the same object model over a USB byte pipe, with a simulated device for the paths hardware can't be made to take: [`web_builder/frontend/src/lib/usb/`](src:builder/app/src/lib/usb)
- The desktop app's transport — `nusb`, hot-plug, and the file-path bulk plane, under the *same* client: [`obc-desktop/src/usb/`](src:apps/obc-desktop/src/usb) and [`lib/desktop/usb.ts`](src:builder/app/src/lib/desktop/usb.ts)
- The browser's flows over that client — sending a map, a route or a firmware image, and the read-only ride export whose device handle has no way to ack: [`web_builder/frontend/src/lib/device/`](src:builder/app/src/lib/device)
- The desktop app's ride library — the visible GPX folder, the internal archive with its index, and the temp-fsync-rename-fsync write the ack waits on: [`obc-desktop/src/rides.rs`](src:apps/obc-desktop/src/rides.rs); the pull that fills it, and the ack list it computes from the disk: [`lib/device/library.ts`](src:builder/app/src/lib/device/library.ts)
- Shared fixtures pinning the byte layouts every implementation must agree on — the firmware, the phone, and the web builder's wasm converter and USB client: [`specs/vectors/`](src:specs/vectors)
- The route and ride formats that cross the link: [Data formats](../formats/)
