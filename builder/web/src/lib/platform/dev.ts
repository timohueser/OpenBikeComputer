// The dev host: `python -m builder.server` on :8000, which is what local
// development has always talked to. This file is a thin adapter — every call
// below adapts the local tools to the same coverage composer the shipped web
// target uses.

import { api } from "../api/client";
import { LINKS } from "../constants";
import type { Platform } from "./types";

async function catalog({ refresh = false }: { refresh?: boolean } = {}): Promise<{ url: string; body: string }> {
    return api.publishedCatalog(refresh);
}

export const platform: Platform = {
    name: "dev",

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

    // A localhost stand-in for the hosted site keeps the site's chrome.
    siteNav: LINKS,
};
