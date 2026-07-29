/**
 * The dev harness: the whole app, with a simulated device plugged into it.
 *
 * A second entry point rather than a flag inside the app, because "this cannot ship" is worth
 * making structural — no tier's build has this file as an input, so the simulated device is not
 * reachable from anything Rollup emits (see `simulated-device.svelte.ts` for the full reasoning).
 *
 * Run it with the dev server: `npm run dev -- --mode web`, then open `/dev-harness/`.
 */

import { mount } from "svelte";
import "leaflet/dist/leaflet.css";
import "../src/styles/app.css";
import App from "../src/App.svelte";
import { builtMap } from "../src/lib/device/built.svelte";
import { deviceHolder } from "../src/lib/device/session.svelte";
import { firmwareCheck } from "../src/lib/firmware/check.svelte";
import { platform, type Platform } from "../src/lib/platform";
import { harnessRideLibrary } from "./ride-library";
import { BUILT_MAP, openSimulatedSession } from "./simulated-device.svelte";

// Claim the shared session before the app mounts: `DeviceStep`'s own `open()` is memoized, so the
// first opener wins and every surface below it talks to the simulated device.
void deviceHolder.open(openSimulatedSession);

// The same claim-first pattern for the ride library: the shipping `platform.rides` is Tauri IPC
// (desktop mode) or absent (web mode), neither of which a browser tab can serve. Overriding the
// platform object *here*, before the app mounts, keeps the seam entirely inside dev-harness/ —
// no dev flag in src/, nothing for the bundle guards to catch — and both its readers (the Rides
// page and the Device page's library badges) pick it up through the ordinary `platform.rides()`.
{
    const seam = platform as { caps: { rideLibrary: boolean }; rides: Platform["rides"] };
    seam.caps.rideLibrary = true;
    seam.rides = async () => harnessRideLibrary();
}

// A published firmware release, simulated the same way the device is (#1002). The real check asks
// the update host, which #773's U3 has not published to yet — so without this the "available"
// state and the prompt that mentions it are unreachable locally, which is exactly the state a
// harness exists to remove. Claim-first again: `ensure` is memoized, so the card and the prompt
// both read this answer and neither makes a request. The simulated device reports `0.4.0+…`, so
// this reads as an update; sending it fails at the download (there is no image behind the URL) and
// the file picker is the path that actually stages here.
void firmwareCheck.ensure({
    fetch: async () =>
        new Response(
            JSON.stringify({
                version: "0.5.0",
                bytes: 812_345,
                sha256: "a".repeat(64),
                url: "https://updates.openbikecomputer.com/fw/v0.5.0/UPDATE.BIN",
            }),
            { status: 200 },
        ),
});

// Pretend a build just finished (E3 #913), so the Map surface offers the row it offers in the app.
// The harness runs the *web* tier, which has no packer and therefore never produces one — but the
// row's gate is the session's `localFileSource`, not a host name, and the simulated session has
// one. So the built-map path is clickable here without a Tauri build or a `.pbf`.
builtMap.note(BUILT_MAP);

mount(App, { target: document.getElementById("app")! });
