// The dev host: `python -m builder.server` on :8000, which is what local
// development has always talked to. This file is a thin adapter — every call
// below adapts the local tools to the same coverage composer the shipped web
// and desktop targets use. Localhost is a secure WebUSB context, so the local
// builder also uses the browser transport offered by the static web host.

import { api } from "../api/client";
import { LINKS } from "../constants";
import type { Platform } from "./types";

async function catalog(): Promise<{ url: string; body: string }> {
    return api.publishedCatalog();
}

export const platform: Platform = {
    name: "dev",
    caps: {
        // Rides still live on the phone or desktop app. USB map/route transfer,
        // however, is a browser feature and works from this localhost host in
        // Chrome and Edge exactly as it does from the published web app.
        rideLibrary: false,
        deviceUsb: true,
        deviceDashboard: false,
    },

    usbViaWebUsb: true,

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

    device: async () => {
        const { openWebUsbSession } = await import("../usb/session.svelte");
        return openWebUsbSession();
    },
    rides: null,

    // A localhost stand-in for the hosted site keeps the site's chrome.
    siteNav: LINKS,
};
