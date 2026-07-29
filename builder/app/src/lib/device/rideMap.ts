/**
 * The logbook map's clustering (#894 ride-library redesign): pure math, no Leaflet.
 *
 * Zoomed out, the all-rides map does not draw twenty overlapping squiggles the size of ants — it
 * draws forest circles with counts, one per area, and a click zooms into that area. Zoomed in past
 * {@link CLUSTER_BELOW_ZOOM}, the circles dissolve into the actual tracks. The two never show at
 * once; the component reads {@link clustersAt} and draws exactly one of the two layers.
 *
 * The grouping is a **grid cluster in projected pixel space**: each ride is reduced to one
 * representative point (the track's midpoint — a ride is a line, and its middle is a better "where
 * was this" than its start, which for loops is the rider's front door), projected to Web-Mercator
 * pixels at the current zoom, and binned into ~{@link DEFAULT_CELL_PX}-pixel cells. Occupied cells
 * that touch (8-neighborhood) merge into one cluster, so a group of rides straddling a cell edge
 * reads as one badge rather than two badges two pixels apart. Because the projection scales with
 * zoom, the same rides separate into more clusters as the map zooms in — the grid *is* the
 * threshold behavior, and the cutover to real tracks is just the last step of it.
 *
 * Everything here is a pure function over `[lat, lon]` arrays so vitest can pin the behavior
 * (membership, counts, merging, the threshold) without a DOM or a tile server.
 */

/** One ride as the map needs it: its key (for hover/click identity) and its preview track. */
export interface RideTrack {
    readonly key: string;
    readonly track: readonly (readonly [number, number])[];
}

/** One badge on the zoomed-out map. */
export interface RideCluster {
    /** The member rides' keys, in input order. */
    readonly keys: readonly string[];
    readonly count: number;
    /** Where the badge sits: the mean of the members' representative points, `[lat, lon]`. */
    readonly center: readonly [number, number];
    /**
     * `[[south, west], [north, east]]` over **every point of every member track** — what a click
     * on the badge zooms to, so the whole rides come into view, not just their midpoints.
     */
    readonly bounds: readonly [readonly [number, number], readonly [number, number]];
}

/** Below this Leaflet zoom the map shows clusters; at or above it, the tracks themselves. */
export const CLUSTER_BELOW_ZOOM = 10;

/** ~60 px cells: two badges can get no closer than a badge's own diameter. */
export const DEFAULT_CELL_PX = 60;

/** Whether `zoom` is in cluster territory. The component's one branch. */
export function clustersAt(zoom: number): boolean {
    return zoom < CLUSTER_BELOW_ZOOM;
}

/** A ride's one point for clustering: the track's midpoint (by index). `null` for an empty track. */
export function representativePoint(
    track: readonly (readonly [number, number])[],
): readonly [number, number] | null {
    if (track.length === 0) return null;
    return track[Math.floor(track.length / 2)];
}

/** Web-Mercator's latitude limit; beyond it the projection diverges. */
const MAX_LAT = 85.05112878;

/**
 * `[lat, lon]` → Web-Mercator pixels at `zoom` (256 px tiles, Leaflet's own convention). Exported
 * for the tests, which assert cell membership in the same space the clustering bins in.
 */
export function projectPx(lat: number, lon: number, zoom: number): [number, number] {
    const scale = 256 * 2 ** zoom;
    const clamped = Math.max(-MAX_LAT, Math.min(MAX_LAT, lat));
    const s = Math.sin((clamped * Math.PI) / 180);
    return [
        ((lon + 180) / 360) * scale,
        (0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)) * scale,
    ];
}

/**
 * Group rides into clusters for one zoom level.
 *
 * Rides with empty tracks are skipped — there is nothing to place. The result is deterministic:
 * clusters come out ordered by their first member's input position, members in input order.
 */
export function clusterRides(
    rides: readonly RideTrack[],
    zoom: number,
    cellPx: number = DEFAULT_CELL_PX,
): RideCluster[] {
    interface Placed {
        readonly ride: RideTrack;
        readonly rep: readonly [number, number];
        readonly cx: number;
        readonly cy: number;
    }
    const placed: Placed[] = [];
    for (const ride of rides) {
        const rep = representativePoint(ride.track);
        if (!rep) continue;
        const [x, y] = projectPx(rep[0], rep[1], zoom);
        placed.push({ ride, rep, cx: Math.floor(x / cellPx), cy: Math.floor(y / cellPx) });
    }
    if (placed.length === 0) return [];

    // Union occupied cells that touch (8-neighborhood), BFS over the occupancy map.
    const byCell = new Map<string, Placed[]>();
    for (const p of placed) {
        const id = `${p.cx}:${p.cy}`;
        const cell = byCell.get(id);
        if (cell) cell.push(p);
        else byCell.set(id, [p]);
    }
    const seen = new Set<string>();
    const clusters: RideCluster[] = [];
    for (const p of placed) {
        const startId = `${p.cx}:${p.cy}`;
        if (seen.has(startId)) continue;
        const members: Placed[] = [];
        const queue = [startId];
        seen.add(startId);
        while (queue.length > 0) {
            const id = queue.pop()!;
            const cell = byCell.get(id)!;
            members.push(...cell);
            const [cx, cy] = id.split(":").map(Number);
            for (let dx = -1; dx <= 1; dx++) {
                for (let dy = -1; dy <= 1; dy++) {
                    const next = `${cx + dx}:${cy + dy}`;
                    if ((dx !== 0 || dy !== 0) && byCell.has(next) && !seen.has(next)) {
                        seen.add(next);
                        queue.push(next);
                    }
                }
            }
        }
        // Input order inside the cluster, so the result never depends on Map iteration details.
        members.sort((a, b) => placed.indexOf(a) - placed.indexOf(b));
        clusters.push(toCluster(members.map((m) => ({ ride: m.ride, rep: m.rep }))));
    }
    return clusters;
}

/**
 * Known limit: a track crossing the antimeridian (lon jumping ±180) makes its cluster's bounds
 * span most of the world, so a badge click zooms way out instead of in. Left as-is deliberately —
 * unwrapping longitudes is real complexity for a case a bikepacking track effectively never hits,
 * and the failure is a wrong zoom, not a wrong ack or a lost ride.
 */
function toCluster(members: ReadonlyArray<{ ride: RideTrack; rep: readonly [number, number] }>): RideCluster {
    let south = Infinity;
    let west = Infinity;
    let north = -Infinity;
    let east = -Infinity;
    let latSum = 0;
    let lonSum = 0;
    for (const { ride, rep } of members) {
        latSum += rep[0];
        lonSum += rep[1];
        for (const [lat, lon] of ride.track) {
            if (lat < south) south = lat;
            if (lat > north) north = lat;
            if (lon < west) west = lon;
            if (lon > east) east = lon;
        }
    }
    return {
        keys: members.map((m) => m.ride.key),
        count: members.length,
        center: [latSum / members.length, lonSum / members.length],
        bounds: [
            [south, west],
            [north, east],
        ],
    };
}
