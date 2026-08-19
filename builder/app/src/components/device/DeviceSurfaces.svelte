<!--
  What you can do with a connected device, in one lazily-loaded piece: three writes, and one read.

  It exists to be a **chunk boundary**. Everything below it reaches the protocol client, the codecs
  and the transport — about 24 kB that C3 deliberately kept out of the entry bundle, and that one
  ordinary-looking static import would drag straight back in (`usb/bundle.test.ts` fails if it
  does). Splitting here rather than at `DeviceStep` keeps the connect button and its gate in the
  entry bundle, where they have to be: they are what a visitor sees before any of this is needed.

  The ride panel here is handed a `RideSource` — two reads, and no way to write — so "this surface
  cannot change anything on the device" is a property of what the panel has rather than of what it
  remembers not to call (#894, C5 #904). The tier with a managed folder (`caps.rideLibrary`) does
  not render these surfaces at all any more: its device features live on the Device and Ride-library
  pages.
-->
<script lang="ts">
    import type { FlatStoreClient } from "../../lib/usb/client";
    import type { DeviceInfo } from "../../lib/usb/records";
    import type { StoreIdentity } from "../../lib/usb/session";
    import { rideAccess, rideScope } from "../../lib/device/rides";
    import FirmwareCard from "./FirmwareCard.svelte";
    import MapSend from "./MapSend.svelte";
    import RideExport from "./RideExport.svelte";
    import RouteDrop from "./RouteDrop.svelte";

    let {
        client,
        info,
        store = null,
    }: {
        client: FlatStoreClient;
        info: DeviceInfo | null;
        store?: StoreIdentity | null;
    } = $props();

    const rides = $derived(rideAccess(client));
    // `(serial, era)` — the id era every ride id is only meaningful inside. A re-initialized card
    // mints a new `StoreId`, and anything the page remembered about a ride id becomes a claim about
    // a different ride.
    const scope = $derived(rideScope(info, store));
</script>

<MapSend {client} />
<RouteDrop {client} />
<RideExport {rides} {scope} />
<FirmwareCard {client} {info} />
