// Joining the catalog onto the map: the manifest speaks in slash-separated
// region ids (`europe/switzerland`), the Geofabrik index the picker draws
// speaks in flat ids plus a `parent` pointer (`switzerland`, parent `europe`).
// One is the path of the other, so the join is a walk up the parent chain.
//
// Regions **nest**, and OBCC §3 is explicit that a consumer MUST NOT assume a
// parent's artifact subsumes a child's or vice versa: `europe/switzerland` and
// `europe/switzerland/ticino` are separate downloads covering different areas
// at different sizes. So nothing here ever substitutes one for another. What it
// does instead is answer the two questions the picker asks when a region has no
// artifact of its own — *is a region containing this one baked?* and *are any
// regions inside it baked?* — and let the UI offer those as named alternatives
// the rider picks deliberately.

import type { RegionFeature } from "../platform/types";
import type { Catalog, CatalogArtifact } from "./manifest";

/** One Geofabrik region, with its catalog identity and its place in the tree. */
export interface RegionEntry {
    /** The Geofabrik feature id — flat, and what the map picker selects. */
    id: string;
    /** The catalog's `region_id`: the parent chain joined with "/". */
    path: string;
    name: string;
    /** Geofabrik id of the containing region, or null at the top. */
    parent: string | null;
    /** Its artifacts, in manifest order (one per preset that was baked). */
    artifacts: CatalogArtifact[];
}

/**
 * The manifest, joined onto the region polygons. Built once per (regions,
 * catalog) pair and then read; nothing here mutates.
 */
export class CatalogIndex {
    readonly catalog: Catalog;
    /** Every region the picker can draw, by Geofabrik id. */
    readonly entries: ReadonlyMap<string, RegionEntry>;
    /** Ids of regions with at least one artifact — the coverage layer. */
    readonly bakedIds: readonly string[];
    /**
     * Region ids in the manifest with no polygon in the Geofabrik index — a
     * published artifact this picker cannot offer, because it has nothing to
     * draw or click. That is a bakery-side mistake (a curated region id that
     * isn't a Geofabrik one), so it is reported rather than dropped: the store
     * warns, and the count is here for whoever asks.
     */
    readonly unmatchedRegionIds: readonly string[];

    private readonly childIds = new Map<string, string[]>();

    constructor(regions: RegionFeature[], catalog: Catalog) {
        this.catalog = catalog;

        const byId = new Map(regions.map((f) => [f.properties.id, f]));
        const pathCache = new Map<string, string>();
        const pathOf = (id: string): string => {
            const cached = pathCache.get(id);
            if (cached) return cached;
            // Walk to the root, guarding against a parent that isn't in the
            // index and against a cycle a malformed index could contain.
            const segments: string[] = [];
            const visited = new Set<string>();
            let cursor: string | null = id;
            while (cursor && byId.has(cursor) && !visited.has(cursor)) {
                visited.add(cursor);
                segments.unshift(cursor);
                cursor = byId.get(cursor)!.properties.parent;
            }
            const path = segments.join("/");
            pathCache.set(id, path);
            return path;
        };

        const byPath = new Map<string, CatalogArtifact[]>();
        for (const a of catalog.artifacts) {
            const list = byPath.get(a.region_id);
            if (list) list.push(a);
            else byPath.set(a.region_id, [a]);
        }

        const entries = new Map<string, RegionEntry>();
        const baked: string[] = [];
        const matchedPaths = new Set<string>();
        for (const f of regions) {
            const { id, name, parent } = f.properties;
            const path = pathOf(id);
            const artifacts = byPath.get(path) ?? [];
            if (artifacts.length) {
                baked.push(id);
                matchedPaths.add(path);
            }
            entries.set(id, { id, path, name, parent, artifacts });
            if (parent) {
                const siblings = this.childIds.get(parent);
                if (siblings) siblings.push(id);
                else this.childIds.set(parent, [id]);
            }
        }

        this.entries = entries;
        this.bakedIds = baked;
        this.unmatchedRegionIds = [...byPath.keys()].filter((p) => !matchedPaths.has(p)).sort();
    }

    get(id: string): RegionEntry | undefined {
        return this.entries.get(id);
    }

    /** Containing regions that have artifacts, nearest first. */
    ancestorsWithArtifacts(id: string): RegionEntry[] {
        const out: RegionEntry[] = [];
        const visited = new Set<string>([id]);
        let cursor = this.entries.get(id)?.parent ?? null;
        while (cursor && !visited.has(cursor)) {
            visited.add(cursor);
            const entry = this.entries.get(cursor);
            if (!entry) break;
            if (entry.artifacts.length) out.push(entry);
            cursor = entry.parent;
        }
        return out;
    }

    /**
     * Regions inside this one that have artifacts. Only the outermost ones: if
     * a sub-region is baked, its own sub-regions are that region's business,
     * not this list's.
     */
    descendantsWithArtifacts(id: string): RegionEntry[] {
        const out: RegionEntry[] = [];
        const queue = [...(this.childIds.get(id) ?? [])];
        const visited = new Set<string>([id]);
        while (queue.length) {
            const next = queue.shift()!;
            if (visited.has(next)) continue;
            visited.add(next);
            const entry = this.entries.get(next);
            if (!entry) continue;
            if (entry.artifacts.length) out.push(entry);
            else queue.push(...(this.childIds.get(next) ?? []));
        }
        return out.sort((a, b) => a.name.localeCompare(b.name));
    }
}
