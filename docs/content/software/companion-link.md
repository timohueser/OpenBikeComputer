---
title: The companion link
description: How protocol v4 moves flat-store objects over BLE and USB binding v5.
---

# The companion link

The companion link moves stored objects between OpenBikeComputer and a client.
BLE and USB use the same protocol-v4 frames.
BLE also supplies pairing, settings, clock, bond removal, and weather-refresh controls.
USB supplies object transfer and device information only.

The normative contracts are:

- [Flat-store protocol v4](src:specs/FLAT_Store_Protocol.md)
- [Flat-store card format](src:specs/FLAT_Store_Format.md)
- [BLE control surface](src:specs/obc-ble-interface-spec.md)

## Two planes: control and data

Protocol v4 has a control channel and a stream channel.
Control frames select an operation and report its result.
Stream frames carry PUT and GET payload bytes.
Only one PUT or GET can be active.

BLE maps control frames to the `objectControl` GATT characteristic.
It maps stream frames to one L2CAP connection-oriented channel (CoC).
The open `protocolVersion` characteristic contains `u16` value 4.
After authentication, the `psm` characteristic identifies the CoC.

USB binding v5 uses one bulk endpoint pair for each plane.
Both transports deliver identical protocol-v4 frame bytes to one transfer engine.

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="BLE uses two protocol-v4 planes. GATT carries control records. L2CAP CoC carries stream records. The phone is the BLE central. The device is the BLE peripheral.">
  <defs>
    <marker id="cl-a" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">Two planes — protocol v4 control and stream</text>

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
  <text class="d-sub" x="360" y="150" text-anchor="middle" style="font-size:9.5px">objectControl · protocolVersion · psm · command · status · config</text>
  <text class="d-sub" x="360" y="168" text-anchor="middle" style="fill:#a9501c">≤ 512 bytes per attribute — a hard wall</text>

  <!-- data lane -->
  <rect class="d-panel-2" x="150" y="190" width="420" height="78" rx="10" />
  <text class="d-label" x="360" y="216" text-anchor="middle" style="fill:#33575b">Data plane · L2CAP CoC</text>
  <text class="d-sub" x="360" y="236" text-anchor="middle">one raw byte pipe · credit-based flow control</text>
  <text class="d-sub" x="360" y="254" text-anchor="middle">stream frames, one transfer at a time</text>

  <!-- connectors -->
  <line class="d-flow" x1="136" y1="131" x2="150" y2="131" marker-start="url(#cl-a)" marker-end="url(#cl-a)" />
  <line class="d-flow" x1="136" y1="229" x2="150" y2="229" marker-start="url(#cl-a)" marker-end="url(#cl-a)" />
  <line class="d-flow" x1="570" y1="131" x2="584" y2="131" marker-start="url(#cl-a)" marker-end="url(#cl-a)" />
  <line class="d-flow" x1="570" y1="229" x2="584" y2="229" marker-start="url(#cl-a)" marker-end="url(#cl-a)" />
</svg>
<figcaption>BLE uses GATT for control records and L2CAP CoC for stream records. USB uses two bulk endpoint pairs.</figcaption>
</figure>

Each control frame has a 16-byte header.
It contains the `OBC4` magic, protocol major, opcode, flags, length, and `RequestId`.
The client selects a nonzero `RequestId`.
The terminal response echoes it.
The same value identifies all stream frames for a PUT or GET.

The adapter delivers the control frame before related stream frames.
It uses link backpressure if a stream frame arrives first.
BLE credits and USB packet completion do not mean that data are durable.
Only a successful store commit makes an upload durable.

## Protocol-v4 operations

| Operation | Function |
| :-- | :-- |
| `LIST` | Read paged catalog entries, `StoreId`, and commit sequence. |
| `STATUS` | Check one known object and revision. |
| `GET` | Download one committed object revision. |
| `PUT` | Create or replace one object with one commit. |
| `REMOVE` | Remove one object head and any retained revision. |
| `CANCEL` | Stop the active PUT or GET. |
| `ARM` | Request validation and installation of an uploaded firmware package. |
| `FORMAT` | Replace the card with a new empty flat store. |

The protocol, clients, and board adapters implement the `ARM` request and response.
The current nRF54LM20 board policy rejects every `ARM` request with `rejected`.
Uploading an update package does not install it.

There is no protocol negotiation, wire minor, session, operation ID, or unsolicited status frame.
A client sends `LIST` before other operations.
A new `StoreId` invalidates all cached catalog data.
A changed commit sequence tells the client to read the catalog again.

## Stored object kinds

| Value | Kind | Payload or function |
| --: | :-- | :-- |
| 1 | Route | OBCR route |
| 2 | Trip | Ordered route membership |
| 3 | Ride | Device-produced recording |
| 4 | Weather bundle | OBCW weather data |
| 5 | Map | One OBCM file with embedded terrain |
| 6 | Retired | Map-set manifest; producers must not write it |
| 7 | Update package | OBCU firmware package |
| 8 | Rollback reserve | Bootloader rollback space |

`ObjectId` and `Revision` are unsigned 64-bit values.
Object IDs are store-global and are not reused.
A create starts at revision 1.
A replace increments the revision.
A LIST entry also supplies kind, flags, length, CRC-32, and a UTF-8 display name.
The display name has a maximum of 48 bytes.

## Transfers and commits

### PUT

A PUT declares the object identity, expected revision, kind, name, length, and CRC-32.
Object ID zero creates an object.
A nonzero ID replaces the expected revision.

The client sends stream frames from absolute offset zero.
Offsets must be contiguous and increasing.
The device writes to an unpublished allocation.
After the final byte, it verifies these items:

- Declared payload length.
- Whole-payload CRC-32/IEEE.
- Validator rules for the object kind.
- Expected revision immediately before commit.

A successful response supplies the object ID, new revision, length, and CRC.
An error makes the new bytes unreachable.
A cancelled or disconnected transfer releases its allocation.
There is no resume or checkpoint operation.

<figure class="fig">
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
<figcaption>A PUT is one request. The device commits only after length, CRC, and kind validation succeed.</figcaption>
</figure>

### GET

A GET selects an object ID and an optional revision.
The device opens that revision and streams bytes in increasing offset order.
The response supplies the served revision, length, and CRC.
The client verifies the complete length and CRC.

### REMOVE and CANCEL

REMOVE uses the object ID and expected head revision.
It removes the head and its retained revision in one commit.
It cannot remove an active recording or reserved object.

CANCEL names the active PUT or GET `RequestId`.
It stops the transfer but does not remove an existing committed object.
A link loss has the same transfer result.

### Reconciliation

Use STATUS after an interrupted replacement.
A committed result confirms the requested revision.
An absent or superseded result means that replacement did not become the head.

A create has no assigned ID before its commit response.
After a lost create response, use LIST.
Match kind, payload length, payload CRC, and display name.
Do not infer state from a notification or operation log.

<figure class="fig">
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
  <text class="d-sub" x="273" y="262" text-anchor="middle" style="fill:#a9501c;font-size:9px">retired — no command</text>
  <rect class="d-hot" x="16" y="252" width="150" height="34" rx="9" style="fill:#f8efe4" />
  <text class="d-sub" x="91" y="273" text-anchor="middle" style="fill:#a9501c">device catalog unchanged</text>
</svg>
<figcaption>LIST supplies the store identity, commit sequence, and catalog entries. Clients use it to reconcile state.</figcaption>
</figure>

Rides become downloadable after the `RECORDING` flag clears.
The iOS client lists finished rides, downloads them, and verifies their CRC.
Protocol v4 has no ride-possession mutation.
The current iOS `ackRides` compatibility method sends no command.
The board does not accept the retired `ackRides` command.

## Pairing and BLE controls

The phone uses LE Secure Connections passkey entry.
The device displays a six-digit passkey.
The rider enters it in the phone system dialog.
This process creates one authenticated bond.

The device stores one phone bond.
While this bond exists, it rejects pairing from a different phone.
The device action **Forget phone** clears the bond.
The bonded phone can also send `forgetBond`.
The Bluetooth power setting does not remove the bond.

Device Information, Battery, and `protocolVersion` are open before pairing.
`psm`, `objectControl`, commands, and configuration require encryption and authentication.
The device also refuses an unencrypted CoC.

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
<figcaption>One passkey creates one bond. Later connections use the stored keys. A bonded device rejects a second phone.</figcaption>
</figure>

BLE keeps a device-local command and configuration surface beside protocol v4.
It supports clock setting, bond removal, weather refresh, and settings.
These controls are not flat-store objects.
They do not exist in USB binding v5.

The phone sets UTC and local offset after encryption.
A GPS fix can also establish trusted UTC.

## Sensors: the device as BLE central

For the phone, the device is a BLE peripheral.
For sensors, the device is a BLE central.
Both roles use one radio at the same time.

The sensor manager supports these standard services:

- Heart Rate Service.
- Cycling Power Service.
- Cycling Speed and Cadence Service.
- Battery Service.

Sensors use their saved address and do not use the phone bond.
The device has one saved slot for heart rate, power, and cadence.
A power meter can supply cadence when no dedicated cadence sensor is configured.

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
<figcaption>The device is a BLE peripheral for the phone and a BLE central for sensors.</figcaption>
</figure>

The manager scans, connects, discovers, subscribes, decodes, and dispatches measurements.
It reconnects after a link failure while Bluetooth is enabled.
A value older than 5 seconds becomes unavailable.
The ride recorder stores fresh sensor samples and summary statistics.
The device does not stream live sensor values to the phone.

## BLE and USB binding differences

| Property | BLE | USB binding v5 |
| :-- | :-- | :-- |
| Protocol frames | Version 4 | Version 4 |
| Control plane | GATT `objectControl` | Control bulk endpoint pair |
| Stream plane | L2CAP CoC | Stream bulk endpoint pair |
| Authorization | Authenticated bond | Physical cable access |
| Device-local controls | Available | Not available |
| Device information | GATT services | EP0 `GET_DEVICE_INFO` |

USB advertises `bInterfaceProtocol = 5` and `bcdDevice = 0x0500`.
A host checks these values before it exchanges a record.
Protocol frames still contain major 4.

Each USB record contains these parts:

1. A little-endian 32-bit record length.
2. Exactly that many protocol-frame bytes.
3. Zero padding to a four-byte boundary.

USB packet boundaries have no record meaning.
A record can span multiple packets.
The stream-record ceiling is 8,208 bytes, including its 16-byte header.
A stream payload is therefore at most 8,192 bytes.
The host-to-device control-record ceiling is 256 bytes.

USB has no mass-storage binding.
The firmware remains the only owner of the card.

## Implementation

- Protocol engine: [`firmware/obc-link/src/flat`](src:firmware/obc-link/src/flat)
- Flat store: [`firmware/obc-storage/src/flat`](src:firmware/obc-storage/src/flat)
- BLE adapter: [`firmware/obc-fw-nrf54l/src/ble`](src:firmware/obc-fw-nrf54l/src/ble)
- USB device adapter: [`firmware/obc-fw-nrf54l/src/usb`](src:firmware/obc-fw-nrf54l/src/usb)
- USB host library: [`host/obc-usb`](src:host/obc-usb)
- iOS protocol client: [`OBCProtocolV4`](src:companion-ios/Packages/OBCKit/Sources/OBCProtocolV4)
- Builder USB client: [`builder/app/src/lib/usb`](src:builder/app/src/lib/usb)
- BLE codecs and sensor decoders: [`obc-ble`](src:firmware/obc-ble)
- Sensor mailbox: [`sensor_hub.rs`](src:firmware/obc-platform/src/sensor_hub.rs)
