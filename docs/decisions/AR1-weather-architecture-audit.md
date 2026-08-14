# AR1 weather architecture audit

Status: architecture input for issues #1256 and #1260, audited at `origin/develop`
`63b935a9` on 2026-08-14. This document records the shipping design; it does not authorize a
behavior change.

## Verdict

Weather has one sound, deliberately specialized repository: `obc-weather` owns format validation,
A/B selection, freshness and the bounded reader/cache; `obc-storage::weather` owns the
transport-neutral inactive-slot transaction. The board owns FAT handles and boot remount, not the
selection policy. AR2 must preserve that split. Routes, maps and weather can share a smaller staged
write/close/sync/commit-marker kernel, but weather is not an immutable catalog object and must not
be forced through `UPLOAD.TMP`.

Two planning assumptions are already obsolete:

- iOS weather is absent from `MainScreenModel`. It has a focused `WeatherSettingsModel`, a narrow
  three-operation `WeatherDeviceLink`, a durable `WeatherJobEngine` actor and a thin
  `WeatherBLEDeviceLink` bridge. AR6 should reduce the remaining weather state inside
  `BLETransport`, not invent another `DeviceWeather` façade.
- The companion request/job extraction planned by AR8 already exists. AR8 should own the pure Rust
  request kernel and thin board/simulator adapters. Companion work belongs to AR6 and must retain
  one queue-confined CoreBluetooth owner.

**Epic gate: NO-GO while this audit PR is unreviewed. GO after it merges and the overlapping
weather-closeout work is idle, subject to the issue-specific dependencies below.** No downstream
change may start from the old MainScreenModel/DeviceWeather assumptions. The on-device proofs in
the final section are gates for the changes that touch static placement, storage durability or data
plane buffering; they are not required for this documentation/test-only change.

At audit time another worktree owns uncommitted closeout changes in companion `OBCWeather` /
`OBCWeatherWire`, formats/docs and `obc-wx-bake` / client surfaces. This audit reads shipping
`origin/develop` only. Work that overlaps those paths is not safe to start until that owner declares
them idle; nothing here claims otherwise.

## End-to-end ownership and data flow

```text
provider adapters
  -> obc-wx-bake canonical frames/mosaic
  -> immutable OBCG shard objects
  -> destination HEAD proof
  -> mutable manifest swap
  -> companion WeatherAssembler (manifest + corridor + hourly point forecast)
  -> canonical OBCW bundle
  -> WeatherJobEngine checkpoint
  -> WeatherDeviceLink one-shot
  -> BLE transfer descriptor + CoC bytes
  -> obc-storage::weather inactive slot (held magic)
  -> closed-file full validation
  -> four-byte magic patch + flush (eligibility point)
  -> board reselects/reopens exact active Candidate
  -> ride-loop WeatherCache / WeatherSnapshot
  -> obc-app dashboard, hourly, rain overlay and alerts
```

The concrete hand-offs are:

1. The baker fetches every configured adapter, derives missing canonical instants and builds one
   priority mosaic ([`canonical.rs` lines 1160-1183](../../host/obc-wx-bake/src/canonical.rs#L1160)).
   It emits non-dry immutable OBCG objects and records their facts in the next manifest
   ([lines 1199-1249](../../host/obc-wx-bake/src/canonical.rs#L1199)). Publishing is four joined
   phases: object batch, per-object destination proof, manifest swap/read-back, then retention sweep
   ([lines 1273-1345](../../host/obc-wx-bake/src/canonical.rs#L1273)). Host operations owns this
   availability/retention contract.
2. `WeatherJobEngine` owns the durable two-connection job and its retry/checkpoint policy
   ([`WeatherJobEngine.swift` lines 26-57](../../companion-ios/Packages/OBCKit/Sources/OBCWeather/Job/WeatherJobEngine.swift#L26)).
   The domain depends on the three bounded operations in `WeatherDeviceLink`, not CoreBluetooth
   ([`WeatherDeviceLink.swift` lines 171-193](../../companion-ios/Packages/OBCKit/Sources/OBCWeather/Job/WeatherDeviceLink.swift#L171)).
   `WeatherBLEDeviceLink` is only wire/domain mapping ([lines 70-149](../../companion-ios/Packages/OBCKit/Sources/OBCTransport/Weather/WeatherBLEDeviceLink.swift#L70)).
3. `BLETransport` remains the sole CoreBluetooth/peripheral/CoC owner. Weather's one-shot read is at
   [`BLETransport.swift` lines 389-598](../../companion-ios/Packages/OBCKit/Sources/OBCTransport/BLE/BLETransport.swift#L389),
   its upload/no-change coordination at [lines 677-1112](../../companion-ios/Packages/OBCKit/Sources/OBCTransport/BLE/BLETransport.swift#L677),
   and the ordinary shared transfer slot/CoC exchange at [lines 927-1045](../../companion-ios/Packages/OBCKit/Sources/OBCTransport/BLE/BLETransport.swift#L927).
4. The board's shared classifier arms type 20 like other uploads, but BLE routes it into the
   weather-specific repository ([`ble/data_plane.rs` lines 125-145](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L125),
   [lines 308-389](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L308)). `ObjectStore` carries
   only the transaction token and maps its typed verdict to the wire
   ([`object_store.rs` lines 1347-1519](../../firmware/obc-fw-nrf54l/src/object_store.rs#L1347)).
5. `obc-storage::weather` selects only an inactive safe target, holds the magic, streams, closes,
   validates and patches the eligibility marker
   ([`weather.rs` lines 22-45](../../firmware/obc-storage/src/weather.rs#L22),
   [lines 85-219](../../firmware/obc-storage/src/weather.rs#L85)). The board adapter implements
   those operations over exact FAT handles and revalidates the selected read handle
   ([`sd.rs` lines 4639-4865](../../firmware/obc-fw-nrf54l/src/sd.rs#L4639)).
6. The ride loop keeps the single resident cache/snapshot, resamples only when its semantic key
   changes, and retries transient bind failures instead of drawing a false dry frame
   ([`ride.rs` lines 714-719](../../firmware/obc-fw-nrf54l/src/ride.rs#L714),
   [lines 1919-1961](../../firmware/obc-fw-nrf54l/src/ride.rs#L1919),
   [lines 2142-2215](../../firmware/obc-fw-nrf54l/src/ride.rs#L2142)). `obc-app` owns display and
   alert policy but deliberately does not own the forecast bytes
   ([`app.rs` lines 1591-1628](../../firmware/obc-app/src/app.rs#L1591)).

## Responsibility owners

| Responsibility | Sole proposed owner | Shipping evidence / boundary |
| --- | --- | --- |
| OBCG/OBCW byte layouts and independent codecs | `obc-formats`, `OBCWeatherWire`, TypeScript client codec | Cross-language independence is Essential oracle diversity; golden vectors are the contract. |
| Slot validation, RFC-1982 generation ordering, exact-tie choice | `obc-weather` | [`slots.rs` lines 89-155](../../firmware/obc-weather/src/slots.rs#L89). No board/simulator copy. |
| Reader/cache, current-frame freshness and fail-closed sampling | `obc-weather` | [`cache.rs` lines 43-187](../../firmware/obc-weather/src/cache.rs#L43). |
| Inactive-slot streamed transaction | `obc-storage::weather` | Six-operation `WeatherSlotIo` port and `WeatherUpload`; no transport names. |
| FAT file handles, sync, card removal and boot re-open | board `sd::Storage` | Adapter only; it calls the shared validation/selector. |
| Request cadence, retry ladder and accepted-upload satisfaction | `obc-ble::DueScheduler`, extended by AR8's pure kernel | Board and simulator already share the scheduler; context construction remains duplicated. |
| Current device facts and rendering/alert policy | `obc-app` plus the board/simulator host adapter | `AppState` adds only rain view controls (`app.rs` 166-184); forecast data stays host-owned. |
| GATT, advertising intent and BLE transport | board BLE adapter | No scheduler/storage policy in the radio lifecycle. |
| Durable fetch/build/deliver job | iOS `OBCWeather.WeatherJobEngine` | No CoreBluetooth dependency. |
| Peripheral, connection, characteristic and CoC ownership | iOS `OBCTransport.BLETransport` | Singular queue-confined engine; AR6 may extract a coordinator over it, never a second owner. |
| Weather settings presentation | `OBCUI.WeatherSettingsModel` | `MainScreenModel` has zero weather identifiers. |
| Dataset publication, atomic manifest and retention | `obc-wx-bake` / host operations | Direct R2/S3 adapter, immutable objects first and manifest last. |
| Simulator HTTP and camera fallback | simulator adapter | AR8 explicitly retains this host-only behavior while sharing semantic context construction. |

`App`/host audit: weather added three rain-view fields to `AppState`, weather-feed invalidation,
alert presentation/policy, ride projection and the neutral request snapshot
([`app.rs` lines 166-184](../../firmware/obc-app/src/app.rs#L166),
[1591-1745](../../firmware/obc-app/src/app.rs#L1591),
[2311-2355](../../firmware/obc-app/src/app.rs#L2311)). It added no weather variant to
`HostCommand` or `HostEvent`; `weatherUnchanged` is a link command owned by the board adapter
([`link/command.rs` lines 195-205](../../firmware/obc-fw-nrf54l/src/link/command.rs#L195)).

## Message boundaries

Capacity is the maximum logically pending value, not a Rust container's byte capacity.

| Producer -> consumer | Owner | Capacity | Required delivery semantics | Shipping shape / action |
| --- | --- | ---: | --- | --- |
| ride loop -> weather due task: device facts | board adapter | 1 latest value | Non-consuming latest read; 1 Hz position updates must not wake, ride/route edges must wake | 48 B blocking `Mutex<Cell<WeatherSnapshot>>` + 12 B `Signal<()>` ([`ble/weather.rs` 50-93](../../firmware/obc-fw-nrf54l/src/ble/weather.rs#L50)); retain this split. |
| Weather screen -> due task: urgent request | request kernel | 1 coalesced level | At least once until consumed; repeated opens coalesce | Atomic level + shared wake (95-102); fold into AR8 kernel input bits. |
| settings commit -> due task | request kernel | 1 edge | Coalesced invalidation; authoritative setting is read from store | Shared wake (127-130); retain signal edge. |
| storage commit -> due task | request kernel | 1 coalesced level | Must not be lost; one commit satisfies the live request | Atomic level + wake (108-114); fold into kernel action. |
| command handler -> due task: unchanged proof | request kernel | 1 live request/reply | Latest only for matching request id; stale id rejected synchronously | `Mutex<Cell<Option<(u32,u16)>>>` + id atomic + wake (116-125). Keep id correlation; consolidate state in AR8. |
| due task -> GATT context | BLE adapter | 1 readable value | Latest live/resting context; authenticated read is the consume boundary | Direct attribute set (211-246); no queue. |
| due task -> advertiser | BLE lifecycle | 1 latest intent | Level plus change edge; a fixed original deadline must survive connect/drop churn | pending + budget + signal ([`ble/state.rs` 150-188](../../firmware/obc-fw-nrf54l/src/ble/state.rs#L150)); AR5/AR8 may replace only as one coherent state. |
| control plane -> data plane: transfer arm | link composition | 1 descriptor | Exactly one active descriptor across both wires; busy is explicit | Per-wire `Signal<Armed>`, shared owner gate; preserve. |
| control plane -> active data plane: abort | link composition | 1 latched edge | Abort targets the wire that owns the descriptor; stale abort drained at close | Per-wire `Signal<()>`; preserve. |
| route search <-> transfer gate | scratch-arena composition | 1 exclusive owner | Atomic mutual exclusion; no check-then-claim split | Current two-atomic `TransferGate` ([`link_gate.rs` 65-145](../../firmware/obc-app/src/link_gate.rs#L65)); replace with one tagged atomic in AR5. |
| BLE/USB runner -> storage transaction | repository | 1 owned session | Linear begin/push/finish-or-abort; stale tokens cannot touch a retry | `WeatherUpload`/other typed sessions, not a generic message bus. |
| storage selection -> ride renderer | board storage adapter | 1 exact candidate + handle | Stable reader remains valid while inactive slot changes; transient rebind fails closed | Session-long active read handle and validation proof; preserve. |
| BLETransport -> WeatherJobEngine: autonomous read | iOS composition | 1 replayed latest event | Completed live contexts replay; resting context filtered; one engine run with merged trigger | `AsyncMulticast`/`AsyncStream` and `WeatherJobBLEBridge` ([bridge 124-149](../../companion-ios/Packages/OBCKit/Sources/OBCTransport/Weather/WeatherBLEDeviceLink.swift#L124)). |
| WeatherJobEngine -> WeatherDeviceLink | weather domain | 1 awaited request/reply | Bounded one-shot, cancel-safe, durable checkpoint before/after external edges | Three async protocol operations; preserve. |
| baker -> object store -> client | host operations | one immutable generation plus one mutable head | Objects durable/fetchable before atomic manifest; no partial generation becomes visible | Joined four-phase publication; preserve. |

Every-item queues elsewhere (`GESTURES`, route/ride delete requests) already use bounded Embassy
`Channel` and remain FIFO. There is no true weather broadcast topology and no justification for a
`PubSubChannel`.

## Storage invariants and recovery characterization

The repository contract is:

1. Both slots are fully inspected with the shared selector before a write. The target is the
   inactive slot; an unreadable possible target is never truncated.
2. The first four bytes are written as zero. Incoming magic stays in the transaction token.
3. The previously active file and its session-long read handle are never opened for write.
4. The wire length and outer CRC must match; then the file is flushed/closed and the entire OBCW is
   validated with an overlay of the held magic.
5. A stale/equal generation never becomes eligible. Generation wrap follows serial arithmetic.
6. Patching and flushing bytes `0..4` is the sole eligibility point. If final sync/close returns an
   ambiguous error, boot may select the old valid slot or the fully persisted new valid slot; it
   must never select partial bytes.
7. Boot and post-commit both run the same selector. The exact selected file is opened read-only and
   revalidated before its identity/handle are published. No valid slot means no weather, not a
   guessed fallback.
8. Freshness/expiry and current-frame choice are reader/app policy, not storage eligibility.

The host-runnable `obc-storage::weather` tests cover successful inactive-only publication,
oversize/append failure and same-boot retry, a power cut at every important header and 512-byte
boundary, outer and embedded CRC corruption, stale/wrapped generations, card removal/full,
unreadable targets, and seek/flush/close ambiguity
([`weather.rs` test module](../../firmware/obc-storage/src/weather.rs#L222)). AR1 adds the missing
characterization that repeatedly validates the old active reader after every inactive-slot append
and again after close failure/reboot. `obc-weather` separately pins deterministic selection,
half-range/wrap rules and overlay validation
([`slots.rs` tests](../../firmware/obc-weather/src/slots.rs#L191)). These are sufficient for AR2 to
extract filesystem ownership without a board test harness; real-media power-loss remains a release
gate for a changed transaction implementation.

## Essential / Policy / Accidental primitive matrix

“Current cost” is target-ABI RAM unless stated otherwise. Candidate sizes came from a temporary
`resource-report` table in a Thumb release build with the repository toolchain; the rows were
removed after extraction. No shipping allocation changed, so this PR's linked RAM/flash/future,
stack, wakes and throughput deltas are all zero. A candidate implementation must re-run the linked
guards because type size alone does not predict async-frame or alignment cost.

| Current mechanism | Cause class and stated cause | Measured comparison | Decision / downstream owner |
| --- | --- | --- | --- |
| Bounded every-item commands | **Essential:** FIFO and explicit backpressure are domain semantics | `Channel<CriticalSectionRawMutex,u16,8>` = 48 B | **Adopt/retain `Channel`**; AR5 names ports/capacities, no generic bus. |
| One-consumer latest/coalescing request | **Essential:** overwriting intermediate requests is the contract | `Signal<()>` = 12 B; `Signal<WeatherSnapshot>` = 48 B | **Adopt `Signal<T>`** where every write may wake. |
| Snapshot level + material-change wake | **Essential:** GPS changes at 1 Hz but the sleeping radio task must have zero idle wakes | current snapshot mutex 48 B + signal 12 B; payload `Signal` 48 B would wake on each update; `Watch<Snapshot,1>` 80 B | **Retain split**. The 12 B apparent saving would change wake behavior. |
| Latest value with independent receivers | **Essential only when N receivers really consume independently** | `Watch<Snapshot,1>` 80 B; `Watch<Snapshot,2>` 88 B | **Adopt only for N>=2**; no current weather boundary qualifies. |
| Coherent synchronous shared state | **Essential:** multi-field snapshot/budget transitions cannot tear | blocking weather snapshot mutex = 48 B, equal to `Signal<Snapshot>` | **Retain blocking `Mutex<Cell/RefCell>`** for synchronous reads. |
| Async SD owner + typed transaction token | **Essential:** one FAT owner/handle may span storage awaits; the token is linear session state | async `Mutex<_,u8>` shell = 20 B; `WeatherUpload` is compile-time capped at <=64 B; shipping token is static because putting it in `ObjectStore` measured boot frame 14,692 -> 27,556 B | **Retain** async owner and typed sessions; never hold across unrelated transport waits. |
| Event-or-deadline sleep | **Essential:** request cadence with zero periodic idle wake | shipping `select(WAKE, Timer)` at `ble/weather.rs` 252-260; guarded poll frame 9,728 B | **Adopt/retain `select` + timer**, no new task/poll. |
| `MaybeUninit` in-place board statics | **Essential:** warm reset may preserve `StaticCell`'s used flag | `MaybeUninit<Snapshot>` 48 B; `StaticCell<Snapshot>` 56 B (+8 B); current boot stack gates below | **Retain** until the exact reset path proves cell re-arming. `ptr::write` is the owner, not boilerplate. |
| Local random-access/FAT transaction traits | **Essential:** `read_at`, root-file identity, closed-file validation and sync/patch ambiguity are not sequential byte-I/O semantics | `WeatherSlotIo` has 6 operations and no adapter allocation | **Retain thin domain port**; AR2 may compose a shared staged kernel underneath it. |
| USB Stage / arena arm | **Accidental:** map throughput work was implemented only in the cable runner | current `arena_usb` 131,072 B but costs 0 incremental resident bytes while tied with `arena_total`; on-glass staged 7.3-7.9 MB/s vs ~0.20 MB/s unstaged | **Unify in #1292/#1296**, preserving DMA/FAT lifetime. Estimate 5-8 engineer-days now plus device soak; continuing divergence costs ~3-5 days/year and repeats correctness fixes. |
| Generic `Pipe` / zerocopy channel proposal | **Essential mismatch:** ordinary rings do not carry FAT span coalescing, aligned arena loans or in-flight CMD25 lifetime | no candidate implementation; current arena arm 128 KiB, guarded resource numbers below | **Reject as drop-in**. Measure only as an implementation inside the staged kernel. |
| Transfer/search gate split across `AtomicU8` + `AtomicBool` | **Accidental:** search was added after the transport-owner gate | current type = 2 B; both check-then-CAS sequences rely on same-executor non-preemption (`link_gate.rs` 67-70) | **Replace with one tagged `AtomicU8`** in AR5: Idle/Ble/Usb/Search. Estimate <=1 day now; annual cost ~1 day plus high-severity race risk whenever an owner is added. |
| Weather scheduler event flags (`URGENT`, `COMMITTED`, id, unchanged slot) | **Accidental spellings around Essential independent facts** | historical weather due plane +408 B resident, including ~62 B event plumbing and ~286 B task future; revision gating +96 B; current guards below | **Fold into AR8's tested kernel state/actions**, but keep the zero-wake snapshot split. Estimate 3-5 days; annual cost ~2-3 days of mirrored board/sim transition changes. |
| Advertising pending + edge + budget | **Essential deadline/level; Accidental three-field representation** | weather request service historically +216 B, including attribute table and intent trio; exact replacement not implemented | **AR5/AR8 may unify as one coherent state only if linked RAM/future stay flat and original deadline survives reconnects.** |
| PubSub/generic event bus | **Accidental complexity with no consumer topology** | no current allocation or true broadcast | **Delete/reject proposal**. |
| Independent Rust/Swift/TypeScript codecs | **Essential:** independent implementations are drift oracles | golden vectors, not LOC minimization | **Retain**, never consolidate across language boundary. |

Planning estimates above are deliberately rough engineering estimates, not performance
measurements. The measured acceptance for every implementation PR is: linked resident/flash,
target type table, task/poll/boot frames, idle wake count, transfer throughput and the relevant
behavioral suites.

## Transport split for #1292

The old USB module header claimed the files differed only at receive/send. AR1 corrects that stale
comment. First, the line-level shared skeleton and physical adapters are:

| Shared semantic skeleton | USB implementation | BLE implementation |
| --- | --- | --- |
| Accept/own one armed descriptor and route by operation | endpoint enable, drain/arm/link-down loop [`usb/data_plane.rs` 293-435](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L293) | encrypted CoC accept and idle-byte/channel reset loop [`ble/data_plane.rs` 45-152](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L45) |
| Terminal result releases owner before reply and drains stale abort | [USB 274-291](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L274) | [BLE 154-169](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L154) |
| Ordinary upload: open, receive/abort, append, finish, reply, store change | [USB 464-819](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L464) | [BLE 187-305](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L187) |
| Download: open, announce, chunk/read/send-or-abort, close, result | [USB 850-965](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L850) | [BLE 391-526](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L391) |
| Echo: receive, running CRC, echo bytes, terminal result | [USB 968-1050](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L968) | [BLE 529-594](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L529) |
| Shared descriptor classification/abort routing/capability refusal | [`link/transfer.rs` 89-364](../../firmware/obc-fw-nrf54l/src/link/transfer.rs#L89) | same function |
| Control-plane framing around the shared classifier | USB frames and status endpoint [`usb/control.rs` 178-393](../../firmware/obc-fw-nrf54l/src/usb/control.rs#L178) | GATT events/status notify [`ble/control.rs` 50-204](../../firmware/obc-fw-nrf54l/src/ble/control.rs#L50) |

The ten semantic divergences that #1292 must act on are exactly 2 Essential, 3 Policy and 5
Accidental:

| Class | USB-only / BLE-only difference and exact lines | Cause | Required action |
| --- | --- | --- | --- |
| **Essential** | USB bulk drain handshake: signals/generation/budgets and drain [`USB 63-272`](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L63), idle select [363-367](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L363), termination drains [538, 619, 677](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L538) | Submitted WebUSB bulk OUT transfers cannot be cancelled and an endpoint has no CoC-close reset. | Retain USB adapter capability; expose one termination hook to shared runner. |
| **Essential** | BLE bounded GATT announce/status notifications [`BLE 424-450`](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L424), [596-628](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L596) | ATT notification can backpressure indefinitely; USB control `send` has endpoint/link failure instead. | Retain BLE adapter deadline capability. |
| **Policy** | USB `MapTarget` variants and commit routing [`USB 438-462`](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L438), [503-526](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L503), [705-759](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L705) | Large maps were declared cable-only and direct-to-final to avoid a second copy/free-space cost. This is product/storage policy, not transport physics. | Re-decide as object/storage strategy in #1292/#1296, applied before transport dispatch. |
| **Policy** | USB maps use `Receiver::new_link_checked` instead of whole-object software CRC [`USB 488-497`](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L488); BLE always uses normal Receiver | Throughput decision based on USB packet CRC + sEMMC CRC/ECC. | One per-object integrity policy shared by both runners; retain exception only with measured CPU/throughput evidence. |
| **Policy** | USB `HeldMagic` for map/set direct-final targets [`USB 498`](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L498), feed [656-663](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L656), patch/commit [716-754](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L716) | Commit-marker policy selected for very large objects. | Move behind the object/storage strategy and staged kernel; do not make it a wire concern. |
| **Accidental** | USB `Stage` buffering [`USB 548-572`](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L548), [658-690](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L658); BLE appends direct | Performance work landed only on the second path. | Shared runner/staged repository kernel; cost 5-8 days now, ~3-5 days/year if retained. |
| **Accidental** | USB scratch-arena request/release [`USB 284-290`](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L284), [548-569](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L548) | Stage was retrofitted after arena composition. | One storage-strategy loan port; cost 2-3 days now, ~1-2 days/year. |
| **Accidental** | USB map on-glass start/progress/end/status [`USB 529-546`](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L529), [680-684](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L680), [761-800](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L761) | Large-map UI was added only where maps were allowed. The event vocabulary itself is not wire-specific. | Shared typed progress sink, with capability policy deciding whether a transfer emits it; cost 1-2 days now, ~1 day/year. |
| **Accidental** | BLE-only duplicate `run_weather_upload` [`BLE 308-389`](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L308) | Weather arrived after the generic runners and was implemented beside them. | One upload runner parameterized by typed repository session; cost 2-3 days now, ~1-2 days/year. |
| **Accidental** | USB-only reject forensics [`USB 765-784`](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L765) | Field investigation instrumentation was added where the failure was observed. | Lift to shared receiver/result diagnostics or delete after a defined retention period; <=1 day now, ~0.5 day/year. |

Physical chunk framing remains in adapters: USB IN is max-packet and terminates exact multiples with
a ZLP ([USB 61](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L61),
[938-948](../../firmware/obc-fw-nrf54l/src/usb/data_plane.rs#L938)); BLE uses a 244-byte CoC SDU
chunk ([BLE 529-530](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L529)) and requests fast
connection parameters ([633-647](../../firmware/obc-fw-nrf54l/src/ble/data_plane.rs#L633)). Those
adapter facts do not license duplicate transaction state machines.

## Resource, API and LOC baseline

Authoritative current board baseline (`firmware/tools/resource_baseline.json`):

| Metric | Current | Guard / context |
| --- | ---: | --- |
| linked resident `.bss + .data` | 308,176 B | 308,256 B max |
| `.uninit` | 132,096 B | includes 131,072 B max-of-arms scratch arena |
| recorded flash | 1,366,436 B default / 1,366,444 B BLE | CI record, host-sensitive |
| largest guarded poll frame | 9,728 B | 12,288 B limit |
| main task body | 7,456 B | 8,192 B limit |
| boot-chain ceiling measured | 14,344 B pinned host / 21,884 B CI | 24,576 B limit |
| residual main stack | 51,248 B | exact current floor; >=4,096 B headroom invariant |
| `WeatherReader` + fixed cache type | 472 B | target ABI; one board instance |
| `App` | 44,384 B | target ABI |
| `ble_total` / `ble_object_store` | 37,798 B / 13,616 B | target ABI |
| `usb_named` / `arena_usb` | 10,908 B / 131,072 B | arena arm is not added to `usb_named` |

Historical same-host deltas explain the current weather cost: WX8 added 408 B resident (~62 B event
plumbing, ~60 B transaction token, ~286 B task future) and about 24 KiB flash; revision gating added
96 B resident, raised the task body 7,328 -> 7,456 B and boot chain 14,216 -> 14,344 B. Both left the
9,728 B poll frame unchanged. These measurements are embedded in the baseline file; they are more
trustworthy than reconstructing a delta from today's unrelated code.

Production PLOC below is a reproducible physical-line baseline: for Rust, lines before the first
top-level `#[cfg(test)]` marker; for board files (which have no test module) and Swift, the whole
production file. Comments/blank lines are intentionally retained, so later splits cannot claim a
win by moving prose into a test module. “Public lines” counts lines beginning with a Rust `pub` or
Swift `public`; protocol requirement counts are listed separately because they are the useful API.

| Surface | production PLOC (physical file total) | public declaration lines / useful API |
| --- | ---: | ---: |
| `obc-storage/src/weather.rs` | 220 (589 before AR1 test) | 16; `WeatherSlotIo` 6 required ops |
| `obc-weather` `lib + slots + cache` | 907 (1,420) | 62 across the three files |
| board `ble/weather.rs` | 306 | 5 adapter entry points |
| board `sd.rs + object_store.rs + ride.rs` | 10,096 | board-private gravity well |
| board BLE data/control | 989 | board-private |
| board USB data/control | 1,477 | board-private |
| `obc-app/src/app.rs` | 3,518 (7,128) | gravity well; weather surface is the methods enumerated above |
| simulator `weather_companion + store + live` | 928 (1,132) | 64 public declaration lines |
| iOS `BLETransport` | 2,950 | 48 public lines; weather concentrated at 389-1112 plus lifecycle callbacks |
| iOS `DeviceTransport` | 305 | 31 protocol requirements, exactly 1 weather requirement (`setWeatherWatch`) |
| iOS `MainScreenModel` | 1,638 | 0 weather identifiers |
| iOS `WeatherSettingsModel` | 345 | 35 public lines |
| iOS `WeatherDeviceLink + JobEngine + BLE bridge` | 933 | `WeatherDeviceLink` exactly 3 requirements |
| host weather `canonical + publish` | 1,777 (2,153) | 116 public declaration lines |
| `host/obc-pack/src/catalog.rs` | 3,130 (3,132) | 206 public declaration lines; AR7 gravity well |

The public-line counts are diagnostic breadth, not an API quality score. AR3/AR6/AR7 must report
the same definitions before/after and list added/removed dependencies; LOC reduction alone does not
justify crossing an ownership boundary.

## Refined downstream scopes and dependencies

| Issue | Refined scope / dependency after AR1 |
| --- | --- |
| **AR2 #1258** | Evidence-based answer: weather **is** a special repository. Preserve `obc-weather` selection/freshness and `obc-storage::weather` transaction; make board FAT ownership explicit. Share only staged stream/close/sync/commit-marker mechanics with #1296. Start after AR1 merge and storage weather-closeout paths are idle; keep the characterization matrix green. |
| **AR3 #1263** | Drop the stale screen-registration seam: the UI already has one screen table/shared vocabulary. Work against `app.rs`'s current 3,518 production-prefix / 7,128 physical lines, not the older 4,480-NLOC claim. Preserve the host-owned weather feed and the small neutral request snapshot; do not move forecast storage into `App`. May run independently after AR1. |
| **AR4 #1262** | `ride.rs` owns the current board weather cache/sampling adapter. AR4 may move that state into `RideRuntime`, but AR8 owns request-kernel semantics and context construction. Agree the seam first; do not duplicate a weather runtime or add a task. Static movement must pass linked and warm-reset gates. |
| **AR5 #1257** | Use the message table above. Keep every-item `Channel`, payload `Signal` only where every update may wake, snapshot mutex+material edge where updates must be quiet, and no `Watch` without multiple receivers. Replace the two-atomic transfer/search gate with one tagged atomic. Coordinate advertising/event consolidation with AR8. |
| **AR6 #1259** | `DeviceWeather` as a new broad capability is obsolete: `WeatherDeviceLink` already supplies the narrow read/upload/unchanged API and `WeatherSettingsModel` is already focused. Keep the one `setWeatherWatch` discovery toggle on `DeviceTransport` until the broader capability split makes a smaller discovery capability useful. Extract BLETransport's 389-1112 weather one-shot/session bookkeeping behind the existing link, without a second CoreBluetooth owner. Start only when companion closeout paths are idle. |
| **AR7 #1261** | Catalog remains independent of weather codecs. Coordinate only the four-phase publication module with #1293; do not merge producer/consumer/oracle validators. The current gravity well is 3,130 production PLOC and one internal import cycle. Avoid active `obc-wx-bake` closeout paths. |
| **AR8 #1287** | Remove the already-completed companion-extraction step. Scope: extend the existing Rust `DueScheduler` into one pure request/context/action kernel; drive board and simulator through it; consolidate eligible board inputs with AR5 measurements. Preserve simulator's host-only HTTP/camera fallback, board zero-idle-wake behavior, and the specialized storage boundary. Rust stages can start after AR1 when board/sim paths are idle; any iOS cleanup is AR6. |

Cross-cutting order: AR1 merge -> #1292/#1296 strategy vocabulary -> AR2 repository composition;
AR5 and AR8 agree the board messages/kernel before either edits `ble/weather.rs`; AR6 alone edits the
companion transport owner; AR3/AR4 agree who moves the ride weather adapter; AR7/#1293 agree the
publication cut before splitting `catalog.rs`.

## Required on-device proofs for downstream changes

This audit PR changes no runtime code, so it needs no device test. The following are explicit gates
for later invasive changes:

1. **Static placement:** warm-reset the exact shipping path repeatedly after initializing each
   candidate `StaticCell`/`ConstStaticCell`; prove no retained used flag/panic, equal alignment/DMA
   address, linked resident delta, task/boot frames and stack high-water. Until then retain
   `MaybeUninit + ptr::write`.
2. **Weather transaction:** on a real card, upload a new bundle while the old one is rendered; cut
   power/remove the card at body, close, magic write, flush and close-return boundaries. On reboot
   observe either the old or fully valid new generation, never no-data/partial, and verify the old
   reader never glitches before activation.
3. **Shared staged runner:** for USB maps and BLE weather/routes, record object bytes/verdicts,
   abort/retry, renderer responsiveness, watchdog/stack high-water, linked RAM/flash/future sizes,
   idle wake count and throughput. Preserve current USB 7.3-7.9 MB/s staged performance and BLE
   connected-time budgets; an unstaged fallback remains correct but is not a performance pass.
4. **Messaging/request kernel:** with RTT/power instrumentation, verify zero weather-task idle wakes
   when not riding, the same request ids/retry instants/context bytes, and no missed urgent, commit,
   unchanged or settings transition under bursty inputs.
