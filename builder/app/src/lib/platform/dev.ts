// The dev host: `python -m builder.server` on :8000, which is what local
// development has always talked to. This file is a thin adapter — every call
// below adapts the local tools to the same coverage composer the shipped web
// and desktop targets use.

import { api } from "../api/client";
import { LINKS } from "../constants";
import type { Platform } from "./types";

async function catalog(): Promise<{ url: string; body: string }> {
    return api.publishedCatalog();
}

export const platform: Platform = {
    name: "dev",
    caps: {
        // The dev server is a build service, not a device host: no USB, and
        // rides live on the phone or the desktop app, never here.
        rideLibrary: false,
        deviceUsb: false,
        deviceDashboard: false,
    },

    // Moot: no USB at all here, so there is no transport to name.
    usbViaWebUsb: false,

    styleEditor: {
        load: () => import("../../routes/Advanced.svelte"),
        presets: () => api.presets(),
        schema: () => api.schema(),
        palette: () => api.palette(),
        preview: {
            status: () => api.previewStatus(),
            pack: (config, signal) => api.packPreview(config, signal),
        },
    },
    catalog,
    catalogFetch: (input, init) => api.catalogFetch(input, init),
    openMapOutput: null,

    device: null,
    rides: null,

    // A localhost stand-in for the hosted site keeps the site's chrome.
    siteNav: LINKS,
};
