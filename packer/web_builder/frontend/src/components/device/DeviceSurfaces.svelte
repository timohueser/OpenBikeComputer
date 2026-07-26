<!--
  The three things you can write to a connected device, in one lazily-loaded piece.

  It exists to be a **chunk boundary**. Everything below it reaches the protocol client, the codecs
  and the transport — about 24 kB that C3 deliberately kept out of the entry bundle, and that one
  ordinary-looking static import would drag straight back in (`usb/bundle.test.ts` fails if it
  does). Splitting here rather than at `DeviceStep` keeps the connect button and its gate in the
  entry bundle, where they have to be: they are what a visitor sees before any of this is needed.
-->
<script lang="ts">
    import type { ProtocolClient } from "../../lib/usb/client";
    import type { DeviceInfo } from "../../lib/usb/transport";
    import type { MapArtifact } from "../../lib/device/write";
    import FirmwareCard from "./FirmwareCard.svelte";
    import MapSend from "./MapSend.svelte";
    import RouteDrop from "./RouteDrop.svelte";

    let {
        client,
        info,
        artifact = null,
    }: { client: ProtocolClient; info: DeviceInfo | null; artifact?: MapArtifact | null } = $props();
</script>

<MapSend {client} {artifact} />
<RouteDrop {client} />
<FirmwareCard {client} {info} />
