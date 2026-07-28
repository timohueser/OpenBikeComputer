// The catalog, loaded once and held for the session.
//
// Two OBCC §7 obligations shape this file. The manifest is read as one whole
// document and either wholly accepted or wholly rejected — `parseCatalog` is
// the only way in, including out of the cache, so a stored copy cannot rot into
// a shape the app would never have accepted fresh. And a rejection **retains
// the previously cached manifest**: a bad publish, a truncated response or a
// CDN hiccup leaves the site showing the last good catalog with a note saying
// so, rather than showing nothing or, worse, a half-populated list.

import type { Preset } from "../config/model";
import { platform } from "../platform";
import type { RegionFeature } from "../platform/types";
import type { DeviceMapSupport } from "./availability";
import { parseCatalog, type Catalog } from "./manifest";
import { catalogPresets } from "./presets";
import { CatalogIndex } from "./regions";

const CACHE_KEY = "obcm.catalog";

interface CachedCatalog {
    fetchedAt: number;
    body: string;
}

export type CatalogState = "idle" | "loading" | "ready" | "error";

export class CatalogStore {
    state = $state<CatalogState>("idle");
    index = $state<CatalogIndex | null>(null);
    /** The style choices, from whichever manifest ended up in play. */
    presets = $state<Preset[]>([]);
    /** Why the live manifest was refused, when a cached one is being shown. */
    staleReason = $state<string | null>(null);
    /** When the shown manifest was fetched, if it came from the cache. */
    cachedAt = $state<number | null>(null);
    /** Fatal load error: no manifest at all, live or cached. */
    error = $state<string | null>(null);

    /**
     * The device the maps are for, or null when none is known — the hosted
     * tier's designed case, and still the common one: nothing plugged in, or a
     * browser with no USB at all. Null is not a degraded mode; it is the branch
     * the spec writes for "no known target firmware".
     *
     * Set by the device step from a real identity read (E1, #911). It stays
     * null for a device whose read carries no `obcm_version` — an older
     * firmware, or the store-less short read — because "unknown" and "reads
     * some particular version" are different answers and only one of them is
     * true.
     */
    device = $state<DeviceMapSupport | null>(null);

    async load(): Promise<void> {
        if (this.state === "loading" || this.state === "ready") return;
        this.state = "loading";

        let regions: RegionFeature[];
        try {
            regions = await platform.regions();
        } catch (e) {
            this.error = `Couldn't load the region map: ${message(e)}`;
            this.state = "error";
            return;
        }

        let catalog: Catalog;
        try {
            catalog = await platform.catalog();
            // Same fetch, already fulfilled: the style list and the artifact
            // list cannot come from two different documents.
            this.presets = await platform.presets();
            this.cache(catalog);
        } catch (e) {
            // §7: keep what was cached rather than degrade to a partial view.
            const cached = this.restore();
            if (!cached) {
                this.error = `Couldn't load the map catalog: ${message(e)}`;
                this.state = "error";
                return;
            }
            catalog = cached.catalog;
            this.presets = catalogPresets(catalog);
            this.staleReason = message(e);
            this.cachedAt = cached.fetchedAt;
        }

        const index = new CatalogIndex(regions, catalog);
        if (index.unmatchedRegionIds.length) {
            // Published artifacts with nowhere on the map to put them: the
            // catalog is fine, the bakery's region ids are not. Loud in the
            // console, because there is no user action behind it.
            console.warn(
                "map catalog: no region polygon for " + index.unmatchedRegionIds.join(", "),
            );
        }
        this.index = index;
        this.state = "ready";
    }

    /** The seam the device step calls with the OBCM version the connected
     *  firmware reads, and with `null` on disconnect / an unknown version. Kept
     *  deliberately independent of C3's `DeviceSession`: the catalog wants one
     *  number, not a connection — which is also what keeps the USB stack out of
     *  the entry bundle, since nothing here imports it. */
    setDevice(device: DeviceMapSupport | null): void {
        this.device = device;
    }

    private cache(catalog: Catalog): void {
        try {
            const entry: CachedCatalog = { fetchedAt: Date.now(), body: JSON.stringify(catalog) };
            localStorage.setItem(CACHE_KEY, JSON.stringify(entry));
        } catch {
            // Quota or private mode: the session still works, it just has no
            // fallback next time.
        }
    }

    private restore(): { catalog: Catalog; fetchedAt: number } | null {
        try {
            const raw = localStorage.getItem(CACHE_KEY);
            if (!raw) return null;
            const entry = JSON.parse(raw) as CachedCatalog;
            // Through the same gate as a fresh body — a cached document gets no
            // shortcut past the checks that admitted it in the first place.
            return { catalog: parseCatalog(entry.body), fetchedAt: entry.fetchedAt };
        } catch {
            localStorage.removeItem(CACHE_KEY);
            return null;
        }
    }
}

function message(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
}

export const catalogStore = new CatalogStore();
