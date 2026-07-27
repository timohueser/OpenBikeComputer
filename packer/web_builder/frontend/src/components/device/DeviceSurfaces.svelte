<!--
  What you can do with a connected device, in one lazily-loaded piece: three writes, and one read.

  It exists to be a **chunk boundary**. Everything below it reaches the protocol client, the codecs
  and the transport — about 24 kB that C3 deliberately kept out of the entry bundle, and that one
  ordinary-looking static import would drag straight back in (`usb/bundle.test.ts` fails if it
  does). Splitting here rather than at `DeviceStep` keeps the connect button and its gate in the
  entry bundle, where they have to be: they are what a visitor sees before any of this is needed.

  This is also the seam where the ride panel's device is narrowed, and the narrowing differs by
  tier because the *rule* does. A tier with no managed folder gets a `RideSource` — two reads, no
  ack — so "the browser never acks" is a property of what the panel has rather than of what it
  remembers not to call (#894, C5 #904). A tier with one gets a `RideSyncSource`: the same two
  reads plus `ackRides`, and nothing else a client also carries. Which of the two is built is
  decided by `platform.caps.rideLibrary` right here, once (E2 #912).
-->
<script lang="ts">
    import type { ProtocolClient } from "../../lib/usb/client";
    import type { VersionRead } from "../../lib/usb/protocol";
    import type { LocalFileSource } from "../../lib/usb/session";
    import type { DeviceInfo } from "../../lib/usb/transport";
    import type { MapArtifact } from "../../lib/device/write";
    import { rideAccess, rideScope } from "../../lib/device/rides";
    import { rideSyncAccess } from "../../lib/device/library";
    import { platform } from "../../lib/platform";
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

    // The library and its panel, on the one tier that has a folder. Both are loaded on demand —
    // the panel drags in the GPX exporter and the library drags in the Tauri ride commands, and a
    // window that only sends a map needs neither. `platform.rides` is null wherever
    // `caps.rideLibrary` is false, so this promise simply never exists there.
    const managed = platform.caps.rideLibrary && platform.rides ? loadLibrary(platform.rides) : null;

    async function loadLibrary(open: NonNullable<typeof platform.rides>) {
        const [library, panel] = await Promise.all([open(), import("./RideLibrary.svelte")]);
        return { library, Panel: panel.default };
    }
</script>

<MapSend {client} {artifact} {localFileSource} />
<RouteDrop {client} />

{#if managed}
    {#await managed}
        <p class="small muted">Opening the ride library…</p>
    {:then { library, Panel }}
        <Panel rides={rideSyncAccess(client)} {library} {scope} />
    {:catch reason}
        <p class="note error small" role="alert">
            The ride library could not be opened ({reason instanceof Error ? reason.message : reason}).
            Rides are still on the device and nothing has been changed there.
        </p>
    {/await}
{:else}
    <RideExport {rides} {scope} />
{/if}

<FirmwareCard {client} {info} />
