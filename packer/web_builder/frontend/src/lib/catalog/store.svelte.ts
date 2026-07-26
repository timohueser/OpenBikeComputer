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
     * The device the maps are for, or null when none is known. C3 (#902) owns
     * the USB session and is what will set this; until then it is null except
     * for the test hook below, and null is the hosted tier's designed case —
     * every path here works with nothing plugged in.
     */
    device = $state<DeviceMapSupport | null>(null);
    /** True when `device` came from the URL hook rather than a real device. */
    deviceIsSimulated = $state(false);

    async load(): Promise<void> {
        if (this.state === "loading" || this.state === "ready") return;
        this.state = "loading";
        this.readDeviceHook();

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

    /** Called by C3 when a device attaches or detaches. */
    setDevice(device: DeviceMapSupport | null): void {
        this.device = device;
        this.deviceIsSimulated = false;
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

    /**
     * `?device-obcm=9` stands in for a connected device until C3 lands, so the
     * unsupported state can be exercised without one. The UI shows a banner
     * whenever it is on, so it can never be mistaken for a real device; C3
     * replaces it with `setDevice()` from a real session.
     */
    private readDeviceHook(): void {
        try {
            const raw = new URLSearchParams(location.search).get("device-obcm");
            if (raw === null) return;
            const obcmVersion = Number.parseInt(raw, 10);
            if (!Number.isInteger(obcmVersion) || obcmVersion < 0) return;
            this.device = { obcmVersion, label: "Simulated device" };
            this.deviceIsSimulated = true;
        } catch {
            // No location (SSR, tests): no hook, which is the normal case.
        }
    }
}

function message(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
}

export const catalogStore = new CatalogStore();
