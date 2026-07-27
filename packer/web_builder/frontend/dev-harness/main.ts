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
import { BUILT_MAP, openSimulatedSession } from "./simulated-device.svelte";

// Claim the shared session before the app mounts: `DeviceStep`'s own `open()` is memoized, so the
// first opener wins and every surface below it talks to the simulated device.
void deviceHolder.open(openSimulatedSession);

// Pretend a build just finished (E3 #913), so the Map surface offers the row it offers in the app.
// The harness runs the *web* tier, which has no packer and therefore never produces one — but the
// row's gate is the session's `localFileSource`, not a host name, and the simulated session has
// one. So the built-map path is clickable here without a Tauri build or a `.pbf`.
builtMap.note(BUILT_MAP);

mount(App, { target: document.getElementById("app")! });
