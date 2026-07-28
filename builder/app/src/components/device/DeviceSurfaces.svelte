<!--
  What you can do with a connected device, in one lazily-loaded piece: three writes, and one read.

  It exists to be a **chunk boundary**. Everything below it reaches the protocol client, the codecs
  and the transport — about 24 kB that C3 deliberately kept out of the entry bundle, and that one
  ordinary-looking static import would drag straight back in (`usb/bundle.test.ts` fails if it
  does). Splitting here rather than at `DeviceStep` keeps the connect button and its gate in the
  entry bundle, where they have to be: they are what a visitor sees before any of this is needed.

  The ride panel here is always the no-folder one: a `RideSource` — two reads, no ack — so "the
  browser never acks" is a property of what the panel has rather than of what it remembers not to
  call (#894, C5 #904). The tier with a managed folder (`caps.rideLibrary`) does not render these
  surfaces at all any more: its device features live on the Device and Ride-library *pages*, and
  its builder column keeps only the map send (`MapSendStep`).
-->
<script lang="ts">
    import type { ProtocolClient } from "../../lib/usb/client";
    import type { VersionRead } from "../../lib/usb/protocol";
    import type { LocalFileSource } from "../../lib/usb/session";
    import type { DeviceInfo } from "../../lib/usb/transport";
    import type { MapArtifact } from "../../lib/device/write";
    import { rideAccess, rideScope } from "../../lib/device/rides";
    import FirmwareCard from "./FirmwareCard.svelte";
    import MapSend from "./MapSend.svelte";
    import RideExport from "./RideExport.svelte";
    import RouteDrop from "./RouteDrop.svelte";

    let {
        client,
        info,
        identity = null,
        artifact = null,
        localFileSource = null,
    }: {
        client: ProtocolClient;
        info: DeviceInfo | null;
        identity?: VersionRead | null;
        artifact?: MapArtifact | null;
        /** The session's disk-to-endpoint path (E3 #913), narrowed to the one surface that has a
         *  local file to send. A route or a firmware image is kilobytes and arrives from a drop,
         *  so neither has anything to gain from it. */
        localFileSource?: LocalFileSource | null;
    } = $props();

    const rides = $derived(rideAccess(client));
    // `(serial, epoch)` — the id era every ride id is only meaningful inside. A card swap changes
    // it, and anything the page remembered about a ride id becomes a claim about a different ride.
    const scope = $derived(rideScope(info, identity));
</script>

<MapSend {client} {artifact} {localFileSource} />
<RouteDrop {client} />
<RideExport {rides} {scope} />
<FirmwareCard {client} {info} />
