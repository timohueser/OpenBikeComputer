# FS10 client acceptance evidence

Inventory for [FS10 #1392](https://github.com/timohueser/OpenBikeComputer/issues/1392).
Audited on 2026-09-16 against source `b765faca19cbd7002e986c757f1e5456a50f52bc`.
This is an evidence index, not a new test result or an FS10 closure. No tests or device sessions
were repeated for this inventory. A pass applies only to the behavior and build recorded below.

## Evidence levels

- **Device:** a reported session with a physical board. Older sessions remain historical evidence.
- **Runtime:** the actual native process or browser page, with a host card. This does not prove USB,
  BLE, SD power-loss behavior, or an iPhone application journey.
- **Component:** codecs, simulated peers, real filesystem tests, or the real engine driven in-process.
  A simulated peer is not the board engine. A successful build or CI run alone is not device evidence.
- **Source:** implemented behavior at the audited commit, with no execution claim.
- **Pending:** the linked record does not establish the required result. This does not assert that
  nobody has ever performed the check.

## Client, transport and object inventory

`PUT`, `GET`, `LIST` and `REMOVE` mean upload, download, catalog read and deletion on the device.
A file import/export in a simulator is a separate boundary. Each row states the evidence limit.

| Client / transport | Object kind | Build or record | Result and remaining scope |
| --- | --- | --- | --- |
| Browser builder / Chrome WebUSB, USB binding v5 | Map (one OBCM object) | [#1459](https://github.com/timohueser/OpenBikeComputer/pull/1459), flashed head `a2cb6c2a` | **Device, historical pass:** 850,824,480-byte PUT, reboot, mount, open and render; 6.528 MB/s device-side. Browser version and independent web build hash are not recorded. Map GET/REMOVE, failed commit and disconnected fresh retry are not established by this run. |
| USB client / board | Route | [FS7 final session](https://github.com/timohueser/OpenBikeComputer/issues/1389#issuecomment-5387699140), 2026-08-23 | **Device, owner-reported pass:** PUT, LIST, open, same-ID replacement and REMOVE refresh. Record says merged `develop`; exact firmware/client builds and which USB shell were used are absent. It does not establish host GET or fault recovery. |
| iOS companion / BLE | Route, trip | [FS7 final session](https://github.com/timohueser/OpenBikeComputer/issues/1389#issuecomment-5387699140), 2026-08-23 | **Device, owner-reported pass:** active-route replacement/removal; trip upload, stage load, rename/reorder replacement and removal. Exact phone and firmware builds are absent. Device open/load does not prove a client GET. Current-build identity and failure cases remain pending. |
| iOS companion / BLE client with simulated peers and local filesystem | Ride | [#1680](https://github.com/timohueser/OpenBikeComputer/pull/1680), head `7ba007de`; [#1699](https://github.com/timohueser/OpenBikeComputer/pull/1699), head `fc7b1462` | **Component pass:** exact GET source tuple, atomic archive, revision changes, corrupt/missing sources, partial batches, receipt delivery and reconnect retry. Device PUT is not a ride operation. Physical BLE GET-to-archive-to-receipt, lost response and phone durability remain pending. Phone deletion is local, not device REMOVE. |
| Browser builder / shared TypeScript USB client | Route, trip, map, finished ride, update | [Client tests](../../../builder/app/src/lib/usb/client.test.ts), [simulated peer](../../../builder/app/src/lib/usb/loopback.ts), audited source | **Source inventory:** modeled PUT/GET/LIST/REMOVE, revision checks, STATUS and cancellation exist. These are not a new passing run and the peer is not the Rust engine. The actual browser builder assembly/download and per-kind USB failure journeys remain pending. Historical device passes are limited to the rows above. |
| Desktop builder / Tauri native USB pipe, shared TypeScript client | Map, route, trip, finished ride, update | [#1454](https://github.com/timohueser/OpenBikeComputer/pull/1454), head `16ac7da7`; [#1620](https://github.com/timohueser/OpenBikeComputer/pull/1620), head `70618500` | **Component pass:** client cutover and full-width ride identity through JSON/index/repair. Real filesystem tests passed; the device-opening test reported no attached device. An actual desktop USB journey on each supported OS remains pending; browser USB evidence does not cover native pipes. |
| Native simulator / Unix persistent host card | Map, route, trip | [#1702](https://github.com/timohueser/OpenBikeComputer/pull/1702), final head `c698a6a8` | **Runtime pass reported:** actual CLI create/refuse/reopen, GPX import/export and exact map/card identity. **Component pass:** complete catalogs, replacement, trip deletion failure/retry. This is direct card access, not USB/BLE client acceptance. The scratch trace is described in the PR; it is not a committed reproducible harness. |
| Native simulator / Unix persistent host card | Ride | [#1718](https://github.com/timohueser/OpenBikeComputer/pull/1718), merged `d00ea1cb` | **Runtime pass reported:** Continue/Save, second process reopen, exact object/samples/totals, and failed optional GPX export preserving the saved ride. **Component pass:** journal/finalization faults and exact damaged deletion. The runtime report names a merge record, not a separate executable hash; scratch scripts were not retained in the repository. |
| Web demo / page-local memory card | Map, route, ride | [#1723](https://github.com/timohueser/OpenBikeComputer/pull/1723), head `39de5e46`, Wasm SHA-256 below | **Runtime pass:** real browser Save, saved detail, saved rides across chapter resets, fresh-page lifetime. Exact identities/sample bytes are component assertions, not browser observations. This page is not the browser builder and has no physical transport. Trips are absent; persistent reload/recovery is unsupported. |
| Shipping board / recorder and normal v4 engine | Ride | [FS8 session](https://github.com/timohueser/OpenBikeComputer/issues/1390#issuecomment-5360229630) and [closeout](https://github.com/timohueser/OpenBikeComputer/issues/1390#issuecomment-5361682391), [#1460](https://github.com/timohueser/OpenBikeComputer/pull/1460), merged `601b08dd` | **Device, historical pass:** recovery → Continue → Save → fresh Start; normal LIST/GET returned exact v3 bytes. The detailed reset transcript predates final fixes and does not identify a shipping phone/client build. It is not proof of current iOS archive receipt or SD supply-cut acceptance. |

The browser artifact recorded in #1723 is
`e996bd11aeada7fc4d20d5656497ea0fb9b1a4818c4a390e04fa5d522034cc30`.
The linked PRs record their checks and omissions. No historical result is silently promoted to the
current source revision. Weather results in FS7 are historical only: weather is absent from the
current [object-kind table](../../../specs/FLAT_Store_Format.md#31-object-kinds).

## Cross-cutting acceptance and gaps

| Requirement | Existing evidence | Remaining acceptance |
| --- | --- | --- |
| Full StoreId, ObjectId and Revision | iOS #1680/#1699 retain the verified source tuple. Desktop #1620 preserves 128-bit StoreId and u64 ObjectId. Host [#1684](https://github.com/timohueser/OpenBikeComputer/pull/1684), head `7b8f1039`, tests identity, pinned revisions and reopen. | Record catalog refresh, card replacement and reinitialization through each shipping client. The desktop [archive request/index](../../../apps/obc-desktop/src/rides.rs) has no Revision field; full object-ID width alone does not establish exact-source archive parity. |
| Unreadable archive index and reserved paths | iOS #1680 refuses unreadable manifests and unknown reserved paths. Desktop #1620 tests unreadable-index file preservation and filename reservation. | Actual application failure/reopen traces and platform durability evidence remain pending. Do not treat an empty catalog or visible file as a persistence receipt. |
| Failed commit and safe removal | [#1688](https://github.com/timohueser/OpenBikeComputer/pull/1688), head `04640eb4`, tests the shared uncertainty fence, pinned readers and remount. [#1701](https://github.com/timohueser/OpenBikeComputer/pull/1701), head `4bf4cf9c`, tests kind-preserving deletion and confirmed absence versus I/O failure. | These are storage/domain component tests. Exercise the actual clients against the real engine: report failure, preserve unrelated objects, remount when required, then retry. No failed I/O may count as absent or fall through to another object kind. |
| Disconnect, fresh retry and reconciliation | iOS [transfer tests](../../../companion-ios/Packages/OBCKit/Tests/OBCProtocolV4Tests/TransferClientTests.swift); TypeScript client/peer tests above. #1699 reports receipt link-loss/card-change tests. | Per-client and per-kind real-engine/transport evidence remains pending. Transfers restart from zero. Reconcile named mutations with STATUS; a lost create needs LIST and its fingerprint because the assigned ID may be unknown. STATUS cannot confirm an archive receipt. |
| Durable ride receipt and partial batches | #1680/#1699 test earlier saves surviving a later failure, unfinished work retry, archive revalidation, and exact ARCHIVE_RIDE responses. #1701 tests device proof consumption and retention policy. | Physical phone-to-board delivery/lost-response/power-loss remains pending. Browser [RideSource](../../../builder/app/src/lib/device/rides.ts) exposes LIST/GET only; a browser download is not archive proof. Desktop local archive writes do not send ARCHIVE_RIDE through that USB path. Retain this explicit gap/disposition before claiming client parity. |
| Runtime convergence | #1702/#1718/#1723 deliver native route/trip/ride and browser recording conversions. #1684 tests Unix persistent create/open and locks. | Windows persistent host-card creation remains unsupported pending directory durability. Memory-card checkpoints report Unsupported for durable recovery. A physical card replacement/power-cut matrix remains with [#1393](https://github.com/timohueser/OpenBikeComputer/issues/1393) and [#1383](https://github.com/timohueser/OpenBikeComputer/issues/1383). |

## Operation dispositions

- **Routes/trips/maps:** client codecs and generic object operations do not by themselves prove a
  shipping UI exposes every operation. Complete the applicable actual-client rows; record deliberate
  unsupported combinations instead of marking a generic codec test as a pass.
- **Rides:** the device recorder creates them; client PUT is refused. Hosted browser ride export is
  read-only LIST/GET with no device REMOVE or archive receipt. Desktop local-library removal is not
  device REMOVE. Companion phone deletion is also local; its [BLE transport](../../../companion-ios/Packages/OBCKit/Sources/OBCTransport/BLE/BLETransport.swift) exposes device deletion for routes/trips, not rides.
- **Metadata and rollback reserves:** device-owned kinds; client PUT is refused. Archive proof enters
  through ARCHIVE_RIDE, not arbitrary metadata upload. Map-set manifests are retired, not pending work.
- **Updates:** client UpdatePackage PUT/ARM code exists, but the shipping
  [BoardPolicy](../../../firmware/obc-fw-nrf54l/src/flat_store.rs) uses the
  [default refusing ARM policy](../../../firmware/obc-link/src/flat/store.rs).
  The live [DFU path](../../../firmware/obc-fw-nrf54l/src/dfu.rs) still scans FAT `UPDATE.BIN`;
  [sd.rs](../../../firmware/obc-fw-nrf54l/src/sd.rs) retains update/rollback storage.
  A loopback ARM pass is not a working board update. [FS9 #1391](https://github.com/timohueser/OpenBikeComputer/issues/1391)
  owns flat-store staging, app-allocated rollback, arm/install/verify and forced-failure rollback.
  Keep the current path until its replacement works.
- **Config/control:** these are separate BLE control surfaces, not an additional flat-store object
  kind. They do not fill a missing object-transfer acceptance row.

## Completion record

For each remaining accepted session, link the client build, firmware build, OS/browser, transport,
card/StoreId, object kind and revision, operation/fault, observed result and retained log or artifact.
State whether the test used a physical board, a host runtime or a simulated peer. Keep a missing
build or artifact explicit. Reuse evidence above where its scope applies; do not repeat broad test
campaigns merely to fill this table. FS10 and the integrated physical gate remain open.
