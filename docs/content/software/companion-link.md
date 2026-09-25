---
title: The companion link
description: How the device and a client move stored objects over Bluetooth and USB.
copy: ai
---

# The companion link

The companion link moves stored objects between the device and a client: routes and maps in, rides
out. Bluetooth and USB carry the same protocol frames, so there is one transfer engine and one set
of rules. Bluetooth adds the device-local controls: pairing, the clock, the settings, and bond
removal. USB carries objects and device information only.

The normative contracts are the [flat-store protocol](src:specs/FLAT_Store_Protocol.md), the
[card format](src:specs/FLAT_Store_Format.md), and the
[BLE control surface](src:specs/obc-ble-interface-spec.md).

## Two planes: control and data

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 414" role="img" aria-label="Protocol-v4 control and stream frames use GATT and L2CAP on BLE, or separate bulk endpoint pairs on USB. Both reach the same device transfer engine. BLE device controls are separate from object transfer.">
  <defs><marker id="software-companion-link-1" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
  <text class="d-tag" x="20" y="26" text-anchor="start">One transfer protocol, two transport bindings</text>
  <text class="d-title" x="20" y="66" text-anchor="start">Binding</text>
  <text class="d-title" x="208" y="66" text-anchor="start">Control frames</text>
  <text class="d-title" x="461" y="66" text-anchor="start">Stream frames</text>
  <rect class="d-panel" x="20" y="86" width="145" height="78" rx="8" />
  <text class="d-title" x="92.5" y="111" text-anchor="middle">BLE</text>
  <text class="d-sub" x="92.5" y="131" text-anchor="middle">Authenticated bond</text>
  <rect class="d-panel" x="190" y="86" width="240" height="78" rx="8" />
  <text class="d-title" x="310" y="111" text-anchor="middle">GATT</text>
  <text class="d-sub" x="310" y="131" text-anchor="middle">objectControl</text>
  <rect class="d-panel" x="455" y="86" width="245" height="78" rx="8" />
  <text class="d-title" x="577.5" y="111" text-anchor="middle">L2CAP CoC</text>
  <text class="d-sub" x="577.5" y="131" text-anchor="middle">PUT / GET payloads</text>
  <rect class="d-panel" x="20" y="184" width="145" height="78" rx="8" />
  <text class="d-title" x="92.5" y="209" text-anchor="middle">USB v5</text>
  <text class="d-sub" x="92.5" y="229" text-anchor="middle">Cable access</text>
  <rect class="d-panel" x="190" y="184" width="240" height="78" rx="8" />
  <text class="d-title" x="310" y="209" text-anchor="middle">Control endpoints</text>
  <text class="d-sub" x="310" y="229" text-anchor="middle">Protocol-v4 records</text>
  <rect class="d-panel" x="455" y="184" width="245" height="78" rx="8" />
  <text class="d-title" x="577.5" y="209" text-anchor="middle">Stream endpoints</text>
  <text class="d-sub" x="577.5" y="229" text-anchor="middle">Protocol-v4 records</text>
  <path class="d-flow" d="M310 262 L310 292" />
  <path class="d-flow" d="M578 262 L578 292" />
<path class="d-flow" d="M310 292 H578" />
  <path class="d-flow" d="M444 292 L444 320" marker-end="url(#software-companion-link-1)" />
  <rect class="d-panel d-focus" x="190" y="322" width="510" height="70" rx="8" />
  <text class="d-title" x="445" y="347" text-anchor="middle">Device transfer engine</text>
  <text class="d-sub" x="445" y="367" text-anchor="middle">A successful commit makes an upload durable</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>BLE and USB carry the same protocol-v4 frames. BLE pairing, clock, and settings controls remain outside this transfer protocol.</figcaption>
</figure>

The protocol has a control channel and a stream channel. Control frames select an operation and
report its result; stream frames carry the payload. Only one transfer is active at a time, which
keeps the device's buffers fixed.

Over Bluetooth the control channel is a GATT characteristic and the stream channel is an L2CAP
channel; over USB each plane is a pair of bulk endpoints. The frame bytes are identical.

The operations are the ones a small object store needs: list the catalog, check one object, get,
put, remove, cancel the active transfer, format the card, and arm an uploaded firmware package.
There is no negotiation, no session, and no unsolicited status frame: a client lists first, and
lists again when the store identity or the commit sequence changes.

Link credits and packet completion do not mean the data is safe. Only a store commit does.

## Objects

The store holds routes, trips, rides, one map, the firmware package, and the rollback reserve.
Object identifiers are never reused; a create starts at revision one and a replace raises the
revision. A catalog entry carries the kind, length, CRC, and display name, so a client can show the
store without downloading anything.

## Transfers and commits

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 372" role="img" aria-label="A PUT has four stages. The client sends a PUT control frame. It sends stream frames on L2CAP CoC. The device verifies length and CRC. The PUT response reports the commit or an error.">
  <defs>
    <marker id="tf-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="tf-c" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7.5" markerHeight="7.5" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">One PUT — request · stream · commit</text>

  <!-- actors -->
  <rect class="d-panel" x="80" y="40" width="140" height="34" rx="9" />
  <text class="d-title" x="150" y="62" text-anchor="middle">companion app</text>
  <rect class="d-panel" x="500" y="40" width="140" height="34" rx="9" />
  <text class="d-title" x="570" y="62" text-anchor="middle">OBC device</text>

  <!-- lifelines -->
  <line x1="150" y1="74" x2="150" y2="352" style="stroke:#9aa884;stroke-width:1.2;stroke-dasharray:4 4" />
  <line x1="570" y1="74" x2="570" y2="352" style="stroke:#9aa884;stroke-width:1.2;stroke-dasharray:4 4" />

  <!-- 1: descriptor -->
  <text class="d-sub" x="360" y="104" text-anchor="middle" style="fill:#3c6b39">1 · PUT control frame — objectControl write</text>
  <text class="d-sub" x="360" y="120" text-anchor="middle">RequestId · kind · ObjectId · length · CRC-32</text>
  <line class="d-flow" x1="150" y1="130" x2="570" y2="130" marker-end="url(#tf-a)" />

  <!-- 2: stream -->
  <text class="d-sub" x="360" y="164" text-anchor="middle" style="fill:#33575b">2 · stream frames — L2CAP CoC</text>
  <line x1="150" y1="176" x2="565" y2="176" style="stroke:#33575b;stroke-width:6;opacity:0.5" marker-end="url(#tf-a)" />

  <!-- device note -->
  <rect class="d-panel-2" x="404" y="196" width="230" height="46" rx="9" style="fill:#f7f4e6" />
  <text class="d-sub" x="519" y="216" text-anchor="middle">sink → storage, running CRC</text>
  <text class="d-sub" x="519" y="232" text-anchor="middle" style="fill:#a9501c">no whole-object buffer</text>

  <!-- 3: verify -->
  <rect class="d-hot" x="470" y="258" width="200" height="42" rx="9" style="fill:#f8efe4" />
  <text class="d-sub" x="570" y="278" text-anchor="middle">3 · final byte →</text>
  <text class="d-sub" x="570" y="294" text-anchor="middle">verify whole-object CRC-32</text>

  <!-- 4: result -->
  <line class="d-hot" x1="570" y1="322" x2="150" y2="322" marker-end="url(#tf-c)" />
  <text class="d-sub" x="360" y="342" text-anchor="middle" style="fill:#a9501c">4 · PUT response: committed or error</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>A PUT is one request. A successful response follows the store commit. The current board checks length and CRC but has no object-specific payload validator.</figcaption>
</figure>

An upload declares the identity, the expected revision, the length, and the CRC before any bytes
move, and the device writes into an unpublished allocation. After the last byte it checks all four
and only then commits. An error or a lost link makes the new bytes unreachable and leaves the old
object exactly as it was. There is no resume: a half-written object is not a state the store can be
in.

The current [board policy](src:firmware/obc-fw-nrf54l/src/flat_store.rs) checks the length and the
CRC, but does not yet parse the payload for its kind, so a successful upload is not proof that the
object can be read.

A download serves one committed revision and reports its length and CRC, and the client verifies
both. Removing an object removes its head and its retained revision in one commit, and cannot
remove an active recording.

### Reconciliation

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 300" role="img" aria-label="The client uses LIST to reconcile the catalog. LIST supplies StoreId, commit sequence, and entries. The client uses GET for required objects. Protocol v4 does not send a ride acknowledgment.">
  <defs>
    <marker id="sy-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="sy-m" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#9aa884" /></marker>
    <marker id="sy-k" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">Reconciliation — LIST identifies the store and catalog</text>

  <rect class="d-panel" x="16" y="70" width="150" height="72" rx="10" />
  <text class="d-sub" x="91" y="42" text-anchor="middle" style="fill:#6b7758">on the device</text>
  <text class="d-label" x="91" y="98" text-anchor="middle">store changes</text>
  <text class="d-sub" x="91" y="116" text-anchor="middle">upload · ride</text>
  <text class="d-sub" x="91" y="132" text-anchor="middle" style="fill:#a9501c">device-side delete</text>

  <rect class="d-panel-2" x="192" y="70" width="162" height="72" rx="10" style="fill:#eef2df" />
  <text class="d-label" x="273" y="98" text-anchor="middle" style="fill:#3c6b39">catalog commit</text>
  <text class="d-sub" x="273" y="118" text-anchor="middle" style="font-size:9.5px">StoreId · sequence</text>
  <text class="d-sub" x="273" y="134" text-anchor="middle">LIST response</text>

  <rect class="d-panel" x="380" y="70" width="150" height="72" rx="10" />
  <text class="d-sub" x="455" y="42" text-anchor="middle" style="fill:#6b7758">on the phone</text>
  <text class="d-label" x="455" y="98" text-anchor="middle">LIST changed →</text>
  <text class="d-sub" x="455" y="118" text-anchor="middle">download the list</text>
  <text class="d-sub" x="455" y="134" text-anchor="middle">paged LIST</text>

  <rect class="d-hot" x="562" y="70" width="142" height="72" rx="10" style="fill:#f8efe4" />
  <text class="d-label" x="633" y="98" text-anchor="middle" style="fill:#a9501c">GET required</text>
  <text class="d-sub" x="633" y="118" text-anchor="middle">objects, on</text>
  <text class="d-sub" x="633" y="134" text-anchor="middle">the stream</text>

  <line class="d-flow" x1="166" y1="106" x2="196" y2="106" marker-end="url(#sy-a)" />
  <line class="d-flow" x1="348" y1="106" x2="378" y2="106" marker-end="url(#sy-a)" />
  <line class="d-flow" x1="530" y1="106" x2="560" y2="106" marker-end="url(#sy-a)" />

  <!-- loop back -->
  <path d="M633 142 C 633 190, 91 190, 91 144" fill="none" stroke="#9aa884" stroke-width="1.4" stroke-dasharray="5 4" marker-end="url(#sy-m)" />
  <text class="d-sub" x="360" y="202" text-anchor="middle" style="fill:#6b7758">on the next audit</text>

  <!-- retired ackRides lane -->
  <line x1="20" y1="216" x2="700" y2="216" style="stroke:#d6cda8;stroke-width:1" />
  <text class="d-tag" x="20" y="242" style="fill:#a9501c">Protocol v4 has no ride-acknowledgment mutation</text>
  <rect class="d-panel" x="380" y="252" width="150" height="34" rx="9" />
  <text class="d-sub" x="455" y="273" text-anchor="middle">phone stores verified ride</text>
  <line x1="378" y1="269" x2="168" y2="269" style="stroke:#cf6a2a;stroke-width:1.6" marker-end="url(#sy-k)" />
  <text class="d-sub" x="273" y="262" text-anchor="middle" style="fill:#a9501c;font-size:9px">ARCHIVE_RIDE</text>
  <rect class="d-hot" x="16" y="252" width="150" height="34" rx="9" style="fill:#f8efe4" />
  <text class="d-sub" x="91" y="273" text-anchor="middle" style="fill:#a9501c">device persists archive proof</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>LIST supplies the store identity, commit sequence, and catalog entries. Clients use it to reconcile state.</figcaption>
</figure>

A reply can be lost after the device has committed, so a client must be able to ask what happened
instead of guessing. After an interrupted replacement, a status request says whether the revision
became the head. After a lost create, whose identifier the client never learned, the client lists
the catalog, matches the kind, length, CRC, and name, and removes the duplicates it finds. A
notification is never evidence.

Rides become downloadable when their recording flag clears. The phone downloads the listed
revision, verifies it, saves it in one atomic archive generation, and reports success only after
the file is durable.

The phone then sends the device proof that it holds that exact finalized ride, and the device
commits and reads back that proof. Proof turns the synced indicator on. It is deliberately not a
deletion trigger: routes and rides stay on the device until the rider removes them, and a full card
offers the rider an age-based cleanup instead of deciding alone. The durable format is in the
[metadata contract](src:specs/Ride_Archive_Metadata.md).

## Pairing and the Bluetooth controls

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 400" role="img" aria-label="Pairing, reconnect, and rejection in three rows. Top row, first pairing, done once: the device shows a six-digit passkey on its screen; the rider reads it and types it into the phone; the two run an LESC elliptic-curve key exchange; both sides store the resulting bond keys. Middle row, every time after, silent: the device advertises with a stable address; the phone recognises that identity from the bond; the two re-encrypt with the stored long-term key and the phone's rotating address is resolved via the stored identity key; the result is a connected, encrypted link with no dialog. Bottom row, reject-when-bonded: a different phone tries to pair while a bond already exists; the device suppresses its passkey and drops the link; the other phone sees only a generic pairing failure; the only way through is to clear the bond on the device, with Forget phone or a factory reset.">
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
  <text class="d-sub" x="503" y="202" text-anchor="middle" style="font-size:12px">re-encrypt · stored LTK</text>
  <text class="d-sub" x="503" y="220" text-anchor="middle" style="font-size:12px">resolve RPA · stored IRK</text>

  <rect class="d-hot" x="622" y="180" width="82" height="66" rx="10" style="fill:#f8efe4" />
  <text class="d-sub" x="663" y="204" text-anchor="middle">connected</text>
  <text class="d-sub" x="663" y="222" text-anchor="middle" style="fill:#a9501c">encrypted</text>
  <text class="d-sub" x="663" y="238" text-anchor="middle">no dialog</text>

  <line class="d-flow" x1="166" y1="213" x2="220" y2="213" marker-end="url(#pk-a)" />
  <line class="d-flow" x1="372" y1="213" x2="426" y2="213" marker-end="url(#pk-a)" />
  <line class="d-flow" x1="578" y1="213" x2="620" y2="213" marker-end="url(#pk-a)" />
  <text class="d-sub" x="360" y="266" text-anchor="middle" style="fill:#4d5b3c">bonded + powered + in range  ⇒  connected + encrypted, no interaction</text>

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
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>One passkey creates one bond. Later connections use the stored keys. A bonded device rejects a second phone.</figcaption>
</figure>

Pairing uses Secure Connections passkey entry: the device shows six digits and the rider types them
on the phone. That produces one authenticated bond, and the device stores one phone. While a bond
exists it refuses a second phone, so a bike computer in a group ride cannot be taken over from the
next wheel.

**Forget phone** removes the stored keys. It writes and verifies the empty persistent slot first,
then removes the host keys, because the order decides what a failure leaves behind. Controller
cleanup has no receipt, so the screen asks for a restart to finish. A disconnect proves nothing
about stored keys. A factory reset runs the same removal, so the setup that follows can pair a
phone again.

Device information, the battery level, and the protocol version are readable before pairing.
Everything else needs an authenticated, encrypted link. The clock is set by the phone or by a GPS
fix; the local offset has to come from the phone or the rider.

## Sensors: the device as central

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="214" y="160" text-anchor="middle" style="font-size:12px;fill:#3c6b39">the phone link</text>

  <!-- sensors (device = central) -->
  <rect class="d-panel-2" x="566" y="70" width="138" height="48" rx="10" />
  <text class="d-label" x="635" y="90" text-anchor="middle" style="font-size:12px">heart-rate strap</text>
  <text class="d-sub" x="635" y="106" text-anchor="middle" style="font-size:12px">HRS · 0x180D</text>

  <rect class="d-panel-2" x="566" y="146" width="138" height="48" rx="10" />
  <text class="d-label" x="635" y="166" text-anchor="middle" style="font-size:12px">power meter</text>
  <text class="d-sub" x="635" y="182" text-anchor="middle" style="font-size:12px">Cycling Power · 0x1818</text>

  <rect class="d-panel-2" x="566" y="222" width="138" height="48" rx="10" />
  <text class="d-label" x="635" y="242" text-anchor="middle" style="font-size:12px">cadence sensor</text>
  <text class="d-sub" x="635" y="258" text-anchor="middle" style="font-size:12px">CSC · 0x1816</text>

  <!-- device -> each sensor -->
  <line class="d-flow" x1="462" y1="150" x2="564" y2="96" style="stroke:#33575b" marker-end="url(#se-c)" />
  <line class="d-flow" x1="462" y1="170" x2="564" y2="170" style="stroke:#33575b" marker-end="url(#se-c)" />
  <line class="d-flow" x1="462" y1="192" x2="564" y2="244" style="stroke:#33575b" marker-end="url(#se-c)" />
  <text class="d-sub" x="524" y="160" text-anchor="middle" style="font-size:12px;fill:#33575b">scan · connect · subscribe</text>

  <!-- bottom band -->
  <rect class="d-panel-2" x="24" y="292" width="680" height="40" rx="9" />
  <text class="d-sub" x="364" y="309" text-anchor="middle" style="font-size:12px">one radio — <tspan style="fill:#a9501c">MPSL time-slices</tspan> the peripheral (phone) and central (sensor) roles; no second radio</text>
  <text class="d-sub" x="364" y="325" text-anchor="middle" style="font-size:12px">sensors are open GATT servers — connected by stored address, <tspan style="fill:#a9501c">no bond</tspan>, one saved slot per quantity</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The device is a BLE peripheral for the phone and a BLE central for sensors.</figcaption>
</figure>

For the phone the device is a peripheral. For sensors it is a central, on the same radio at the
same time. It supports the standard heart rate, cycling power, speed and cadence, and battery
services, with one saved slot each for heart rate, power, and cadence.

Sensors use their own saved address and have nothing to do with the phone bond. A reading older
than a few seconds becomes unavailable rather than stale, and the recorder stores the fresh samples
with the ride. The device does not stream live sensor values to the phone.

## Where the two transports differ

| Property | Bluetooth | USB |
| :-- | :-- | :-- |
| Protocol frames | Same | Same |
| Authorization | Authenticated bond | Physical cable access |
| Device-local controls | Available | Not available |
| Device information | GATT services | Control request |

USB frames travel inside length-prefixed records, and a record can span packets. There is no
mass-storage mode: the firmware stays the only owner of the card, so a computer can never leave the
card in a state the firmware did not make.

## Implementation

- Protocol engine: [`obc-link`](src:firmware/obc-link/src/flat)
- Flat store: [`obc-storage`](src:firmware/obc-storage/src/flat)
- BLE and USB adapters: [`ble`](src:firmware/obc-fw-nrf54l/src/ble), [`usb`](src:firmware/obc-fw-nrf54l/src/usb)
- USB host library: [`obc-usb`](src:host/obc-usb)
- iOS protocol client: [`OBCProtocolV4`](src:companion-ios/Packages/OBCKit/Sources/OBCProtocolV4)
- Builder USB client: [`builder/app/src/lib/usb`](src:builder/app/src/lib/usb)
- BLE codecs and sensor decoders: [`obc-ble`](src:firmware/obc-ble)
