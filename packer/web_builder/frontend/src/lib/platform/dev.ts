// The dev host: `python -m packer.web_builder` on :8000, which is what local
// development has always talked to. This file is a thin adapter — every call
// below is the same request today's code made, so the pick-region → edit style
// → build → download loop is unchanged by the platform seam.

import { api } from "../api/client";
import { JobTracker } from "../api/jobs.svelte";
import { PlatformNotImplemented, type LoadStyleEditor, type Platform } from "./types";

export const platform: Platform = {
    name: "dev",
    caps: {
        build: true,
        bboxCrop: true,
        styleEditor: true,
        // The dev server is a build service, not a device host: no USB, and
        // rides live on the phone or the desktop app, never here.
        rideLibrary: false,
        deviceUsb: false,
        deviceDashboard: false,
    },

    regions: () => api.regions(),
    presets: () => api.presets(),
    schema: () => api.schema(),
    palette: () => api.palette(),
    // A build-anything server has no use for pre-baked maps, but the catalog
    // format has to exist before that can be said in code. `async` so it
    // rejects rather than throwing past a caller's `.catch()`.
    catalog: async () => {
        throw new PlatformNotImplemented("dev", "catalog", "A3 #897");
    },

    buildMap: () => new JobTracker(),
    device: null,
    rides: null,

    legacyConfig: () => api.legacyConfig(),
};

export const loadStyleEditor: LoadStyleEditor | null = () => import("../../routes/Advanced.svelte");
