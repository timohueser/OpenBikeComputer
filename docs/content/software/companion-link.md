---
title: The companion link
description: How the OpenBikeComputer device and its phone companion app talk over Bluetooth Low Energy — the two-plane GATT / L2CAP split, the object model, a whole-object transfer end to end, the digest-driven sync loop, and passkey pairing with silent reconnect.
---

# The companion link

The device is a self-contained navigator, but a route is usually *planned* on a
phone and a ride is worth keeping once it's ridden. A small **iOS companion app**
bridges the two over **Bluetooth Low Energy**: push a planned route to the
device, pull tracked rides back, rename the device, read its diagnostics. Once
you've paired, powered, and are in range, it just works — no accounts, no cloud,
nothing leaves the two devices.

This page is the *shape* of that link. The normative, byte-level reference is the
[BLE interface spec](src:obc-ble-interface-spec.md) (the same tier as the
[`OBCM`](src:OBCM_Spec.md) / [`OBCR`](src:OBCR_Spec.md) format specs); here we
cover the design and the *why*. Four ideas run through all of it:

- **Two planes.** Small typed control state rides GATT; bulk bytes ride a single
  L2CAP channel. Nothing large ever crosses GATT.
- **Objects are files the device already speaks.** A route crosses the wire as
  an [OBCR](../formats/) file and is written to storage verbatim; the phone does
  every format conversion, so **the device never parses XML**.
- **One CRC, end to end.** A whole-object checksum is verified once, at commit —
  the check the on-air link CRC can't give you.
- **Interrupted transfers restart, not resume.** Objects are small enough that
  re-sending one whole is simpler and safer than continuing from an offset.

## Two planes: control and data

A BLE **GATT attribute is capped at 512 bytes** — a hard wall, not a soft
budget. A route is tens of kilobytes. So the link is split in two: GATT carries
the small, typed *control* state (identity, config, the orchestration of a
transfer, notifications), and a single **L2CAP connection-oriented channel
(CoC)** is the bulk *data* pipe. GATT says *what is about to happen and how it
went*; the CoC carries *the bytes*.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="The link split into two planes between the companion app on the left (BLE central) and the OBC device on the right (BLE peripheral). The top lane is the control plane over GATT — small typed characteristics: command, status, objectStore, config, transferControl, psm, protocolVersion — with a note that each attribute is capped at 512 bytes. The bottom lane is the data plane over a single L2CAP connection-oriented channel: one raw byte pipe with credit-based flow control, carrying bulk objects one transfer at a time.">
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
  <text class="d-sub" x="360" y="150" text-anchor="middle">command · status · objectStore · config · transferControl · psm</text>
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

## Objects are files the device already speaks

Every bulk payload is a typed **object**. The set is small and closed:

| `type` | Object | Direction | Payload |
|--------|--------|-----------|---------|
| `1` | `route` | app → device (upload) · device → app (detail read) | an [OBCR](../formats/) route file, verbatim |
| `2` | `ride` | device → app | the compact ride object (a tracked ride) |
| `4` | `diagnostics` | device → app | an opaque text blob (boot count, link + storage counters, stack high-water…) |
| `6` / `7` | `routeList` / `rideList` | device → app | the store catalogs — fixed 72-byte entries |
| `3` | `config` | — | reserved on the CoC; the Config blob crosses GATT |

The key move is that **a route on the wire is the same bytes as a route on the
card.** The phone converts an imported GPX or TCX to an OBCR file and streams
that; the device writes it to storage byte-for-byte and later serves it back the
same way. There is no separate "detail" codec — the app's route-detail screen
decodes the very OBCR bytes it uploaded. One layout, one truth. A tracked ride
is the mirror in the other direction: the device stores each finished ride as
the exact bytes it will later stream, so a ride download is a verbatim file copy.

**Object ids are durable.** Each stored object has a `u16` id the device assigns
and keeps **stable across reboots** — the reference firmware encodes it right in
the filename (`RT{id}.OBR` for routes, `RD{id}.ORD` for rides). Durability is
what lets the phone remember *"I uploaded route 7"* and later ask *"is 7 still
there?"* or replace it in place — and it's what a ride sync's
already-have-this-one set keys on.

## A transfer, end to end

Every bulk exchange is the same three-beat shape: **announce over GATT, stream
over the CoC, confirm over GATT.** Here is an upload — a route leaving the phone:

<figure class="fig">
<svg viewBox="0 0 720 372" role="img" aria-label="A sequence diagram of an upload between the companion app on the left and the OBC device on the right. Step one: the app writes a 16-byte transferControl descriptor over GATT — op equals upload, plus type, object id, total length and CRC-32 — which announces the transfer. Step two: the app streams the object's raw bytes over the L2CAP CoC. On the device side a note reads: sink to storage, updating a running CRC, with no reassembly buffer. Step three: once all bytes are in, the device verifies the whole-object CRC. Step four: the device notifies a transferResult over GATT — committed on a match, or crcMismatch which rejects the object so nothing is stored.">
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
  <text class="d-sub" x="360" y="104" text-anchor="middle" style="fill:#3c6b39">1 · transferControl (16 B) — GATT write</text>
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
<figcaption>The descriptor names the transfer (and carries the whole-object CRC); the CoC carries the payload; the <code>transferResult</code> closes it. A fresh upload sends object id <code>0xFFFF</code> ("new") and the device reports the <b>assigned</b> id in the result. A <b>download</b> is the exact mirror: the app asks with an <code>op=download</code> descriptor, the device <em>answers</em> with a descriptor (now carrying the size + CRC) and streams the object back.</figcaption>
</figure>

The checksum is a **whole-object CRC-32/IEEE**, verified once at commit — the
same variant as gzip/PNG. It is deliberately *not* a per-packet CRC (the link
already covers the air); it catches what the link can't — an encode bug, a
storage error — **end to end**, from the phone's encoder to the device's flash
and back.

> **Restart, not resume.** An object is tens of kilobytes — a couple of seconds
> on the wire — so a dropped or aborted transfer is simply re-sent (or
> re-requested) *whole*, never continued from a durable offset. The device
> discards a partial upload the moment the link drops or an `abort` arrives; a
> non-zero offset is rejected outright. (A suffix couldn't be checked against the
> whole-object CRC anyway.) A multi-object flow — syncing several rides — resumes
> at **whole-object granularity**: the rides that fully landed are kept, and the
> rest re-send from byte zero.

> **Full means full — up front.** The device holds a bounded route catalog (64
> routes). A **new**-route upload that would overflow it is refused the instant the
> descriptor arrives — *before any bytes stream* — with a distinct `storageFull`
> result, so the phone can tell the rider to delete routes on the device rather
> than wait out a doomed transfer. Re-uploading an *existing* route (a replace by
> id) is exempt: it reuses a slot rather than growing the catalog, so updating the
> route you're actively navigating never hits the cap.

## Staying in sync — the change digest

After anything changes on the device — a route uploaded, a ride finished, an
object deleted — the phone needs to know *what to re-fetch*, cheaply. Re-reading
the full catalogs on every reconnect would burn the CoC for nothing. So the
device publishes a tiny **10-byte digest**: a `revision` counter plus the route
and ride counts, on a characteristic the app can read *and* subscribe to.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="The sync loop as four stages left to right. One: a store change on the device — an upload commits, a ride is tracked, or an object is deleted. Two: the device bumps the revision in its 10-byte objectStore digest and notifies it. Three: the app compares the digest; if the revision moved it downloads the relevant list object — routeList or rideList — over the CoC. Four: the app fetches only the objects that actually changed. A curved arrow returns from stage four to stage one, labelled on the next change, showing the loop.">
  <defs>
    <marker id="sy-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="sy-m" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#9aa884" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">The change signal — read the digest, fetch what moved</text>

  <rect class="d-panel" x="16" y="70" width="150" height="72" rx="10" />
  <text class="d-sub" x="91" y="42" text-anchor="middle" style="fill:#6b7758">on the device</text>
  <text class="d-label" x="91" y="100" text-anchor="middle">store changes</text>
  <text class="d-sub" x="91" y="120" text-anchor="middle">upload · ride · delete</text>

  <rect class="d-panel-2" x="198" y="70" width="150" height="72" rx="10" style="fill:#eef2df" />
  <text class="d-label" x="273" y="98" text-anchor="middle" style="fill:#3c6b39">revision ++</text>
  <text class="d-sub" x="273" y="118" text-anchor="middle">10-byte digest</text>
  <text class="d-sub" x="273" y="134" text-anchor="middle">notify (GATT)</text>

  <rect class="d-panel" x="380" y="70" width="150" height="72" rx="10" />
  <text class="d-sub" x="455" y="42" text-anchor="middle" style="fill:#6b7758">on the phone</text>
  <text class="d-label" x="455" y="98" text-anchor="middle">digest moved?</text>
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
  <path d="M633 142 C 633 210, 91 210, 91 144" fill="none" stroke="#9aa884" stroke-width="1.4" stroke-dasharray="5 4" marker-end="url(#sy-m)" />
  <text class="d-sub" x="360" y="228" text-anchor="middle" style="fill:#6b7758">on the next change</text>
</svg>
<figcaption>The digest is the cheap "did anything change?" signal that replaces polling the CoC-sized lists. The app reads it on connect and subscribes to it; when the <code>revision</code> moves it pulls the relevant <b>list</b> object (a compact catalog of fixed-size entries), then downloads only the objects that are new to it. A companion <code>storeChanged</code> notification additionally says <em>which</em> store moved, so a route upload doesn't trigger a ride re-list.</figcaption>
</figure>

## Pairing, and staying paired

Access is gated by a **bond** — a one-time, mutually-authenticated pairing. The
device is a *display-only* peer: it shows a **6-digit passkey** on its screen
that the rider types into the phone's system dialog. That's LE Secure
Connections passkey entry — man-in-the-middle-protected — and the on-screen code
is what makes it safe: **physical possession of the device is the control.**
There is exactly one bonded peer, and because seeing the screen is the gate, a
fresh passkey pairing simply *replaces* the stored bond — there's no separate
"clear bond" gesture to hunt for.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="Pairing and reconnect in two rows. Top row, first pairing, done once: the device shows a six-digit passkey on its screen; the rider reads it and types it into the phone; the two run an LESC elliptic-curve key exchange; both sides store the resulting bond keys. Bottom row, every time after, silent: the device advertises with a stable address; the phone recognises that identity from the bond; the two re-encrypt with the stored long-term key and the phone's rotating address is resolved via the stored identity key; the result is a connected, encrypted link with no dialog.">
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

  <rect class="d-panel-2" x="428" y="180" width="150" height="66" rx="10" style="fill:#eef2df" />
  <text class="d-sub" x="503" y="202" text-anchor="middle">re-encrypt · stored LTK</text>
  <text class="d-sub" x="503" y="220" text-anchor="middle">resolve RPA · stored IRK</text>

  <rect class="d-hot" x="622" y="180" width="82" height="66" rx="10" style="fill:#f8efe4" />
  <text class="d-sub" x="663" y="204" text-anchor="middle">connected</text>
  <text class="d-sub" x="663" y="222" text-anchor="middle" style="fill:#a9501c">encrypted</text>
  <text class="d-sub" x="663" y="238" text-anchor="middle">no dialog</text>

  <line class="d-flow" x1="166" y1="213" x2="220" y2="213" marker-end="url(#pk-a)" />
  <line class="d-flow" x1="372" y1="213" x2="426" y2="213" marker-end="url(#pk-a)" />
  <line class="d-flow" x1="578" y1="213" x2="620" y2="213" marker-end="url(#pk-a)" />
  <text class="d-sub" x="360" y="278" text-anchor="middle" style="fill:#6b7758">bonded + powered + in range  ⇒  connected + encrypted, no interaction</text>
</svg>
<figcaption>Pairing happens once, with the passkey on the glass. After that the device keeps a <b>stable</b> address (no device-side privacy rotation), so the phone — which stored that identity at bonding — reconnects silently on any contact and re-encrypts with the stored long-term key; the phone's own rotating address is resolved back to it via the stored identity key. The bond lives in the device's persistent settings storage, so it survives power cycles <em>and firmware updates</em>.</figcaption>
</figure>

What the bond protects, and what stays open:

| Surface | Before pairing |
|---------|----------------|
| Device Information · Battery · `protocolVersion` | **open** — so the app can identity- and version-check *before* pairing |
| every other OBC Control characteristic | **denied** — needs the encrypted, authenticated link |
| the L2CAP CoC | **denied** — opening it on an unencrypted link is refused |

Leaving identity and the protocol version readable pre-bond is deliberate: the
app checks it's talking to a compatible device (and surfaces a mismatch as a
banner rather than trapping) before it ever asks to pair.

---

## Where this lives

- The wire contract, normative: [`obc-ble-interface-spec.md`](src:obc-ble-interface-spec.md)
- The host-tested, radio-free core — descriptor codecs, CRC-32, the transfer state machine: [`obc-ble`](src:firmware/obc-ble) ([`transfer.rs`](src:firmware/obc-ble/src/transfer.rs) · [`descriptor.rs`](src:firmware/obc-ble/src/descriptor.rs))
- On the device — the GATT server, connection lifecycle, and the CoC data plane: [`obc-fw-nrf54l/src/ble/`](src:firmware/obc-fw-nrf54l/src/ble)
- The phone side — the SwiftUI companion app and its transport layer: [`companion-ios/`](src:companion-ios)
- Shared fixtures pinning the byte layouts on both sides: [`protocol-vectors/`](src:protocol-vectors)
- The route and ride formats that cross the link: [Data formats](../formats/)
