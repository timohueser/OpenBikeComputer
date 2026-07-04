// Pure region geometry (no Leaflet): point-in-polygon hit-testing and the
// bbox -> covering-regions sampler. Ported unchanged from the legacy app.js.

import type { RegionFeature } from "../api/client";

export interface IndexedRegion extends RegionFeature {
    _bbox: [number, number, number, number];
    _area: number;
}

export function indexRegions(features: RegionFeature[]): IndexedRegion[] {
    return features.map((f) => {
        const bbox = bboxOf(f);
        return Object.assign(f, {
            _bbox: bbox,
            _area: (bbox[2] - bbox[0]) * (bbox[3] - bbox[1]),
        });
    });
}

export function bboxOf(feature: RegionFeature): [number, number, number, number] {
    let minx = Infinity,
        miny = Infinity,
        maxx = -Infinity,
        maxy = -Infinity;
    const scan = (coords: unknown[]) => {
        for (const c of coords as (number[] | unknown[])[]) {
            if (typeof (c as number[])[0] === "number") {
                const p = c as number[];
                if (p[0] < minx) minx = p[0];
                if (p[0] > maxx) maxx = p[0];
                if (p[1] < miny) miny = p[1];
                if (p[1] > maxy) maxy = p[1];
            } else {
                scan(c as unknown[]);
            }
        }
    };
    scan(feature.geometry.coordinates as unknown[]);
    return [minx, miny, maxx, maxy];
}

function pointInRing(x: number, y: number, ring: number[][]): boolean {
    let inside = false;
    for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
        const xi = ring[i][0],
            yi = ring[i][1],
            xj = ring[j][0],
            yj = ring[j][1];
        if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) {
            inside = !inside;
        }
    }
    return inside;
}

function pointInPolygon(x: number, y: number, polygon: number[][][]): boolean {
    if (!pointInRing(x, y, polygon[0])) return false;
    for (let k = 1; k < polygon.length; k++) {
        if (pointInRing(x, y, polygon[k])) return false; // inside a hole
    }
    return true;
}

export function featureContains(f: IndexedRegion, lng: number, lat: number): boolean {
    const [minx, miny, maxx, maxy] = f._bbox;
    if (lng < minx || lng > maxx || lat < miny || lat > maxy) return false;
    const g = f.geometry;
    if (g.type === "Polygon") return pointInPolygon(lng, lat, g.coordinates as number[][][]);
    if (g.type === "MultiPolygon") {
        return (g.coordinates as number[][][][]).some((p) => pointInPolygon(lng, lat, p));
    }
    return false;
}

export function smallestRegionAt(regions: IndexedRegion[], lng: number, lat: number) {
    let best: IndexedRegion | null = null;
    for (const f of regions) {
        if (!featureContains(f, lng, lat)) continue;
        if (!best || f._area < best._area) best = f; // most specific wins
    }
    return best;
}

// Smallest *leaf* (most specific, downloadable) region at a point. Coarse
// parent regions are skipped: their children tile the same land, and unlike a
// parent's simplified outline a leaf doesn't sprawl across the surrounding sea
// — so a sea point matches no leaf and correctly drags in nothing.
export function smallestLeafAt(regions: IndexedRegion[], lng: number, lat: number) {
    let best: IndexedRegion | null = null;
    for (const f of regions) {
        if (f.properties.has_children) continue;
        if (!featureContains(f, lng, lat)) continue;
        if (!best || f._area < best._area) best = f;
    }
    return best;
}

/**
 * The Geofabrik regions whose PBFs cover a drawn box: union of the smallest
 * leaf region under a 6x6 grid of sample points (sea points match nothing).
 * Only if the box samples no land at all does it fall back to the smallest
 * leaf whose bbox overlaps the box.
 */
export function regionsForBbox(
    regions: IndexedRegion[],
    bbox: [number, number, number, number],
): IndexedRegion[] {
    const [w, s, e, n] = bbox;
    const N = 6; // dense enough to catch small regions inside the box
    const set = new Map<string, IndexedRegion>();
    for (let i = 0; i < N; i++) {
        for (let j = 0; j < N; j++) {
            const x = w + ((e - w) * i) / (N - 1);
            const y = s + ((n - s) * j) / (N - 1);
            const f = smallestLeafAt(regions, x, y);
            if (f) set.set(f.properties.id, f);
        }
    }
    if (set.size) return [...set.values()];

    const overlaps = regions
        .filter((f) => !f.properties.has_children)
        .filter((f) => {
            const b = f._bbox;
            return b[0] <= e && b[2] >= w && b[1] <= n && b[3] >= s;
        })
        .sort((a, b) => a._area - b._area);
    return overlaps.length ? [overlaps[0]] : [];
}

/** Approximate area of a lon/lat box in km² (a size hint, not survey-grade). */
export function bboxAreaKm2(w: number, s: number, e: number, n: number): string {
    const latMid = (((s + n) / 2) * Math.PI) / 180;
    const area = Math.abs(n - s) * 110.574 * Math.abs(e - w) * 111.32 * Math.cos(latMid);
    if (area >= 1000) return Math.round(area).toLocaleString() + " km²";
    return area.toFixed(area < 10 ? 1 : 0) + " km²";
}
