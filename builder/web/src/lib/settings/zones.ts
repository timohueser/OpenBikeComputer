/** The heart rate and power zones of a rider, from the maximum heart rate and the threshold power. */

/** One zone in whole bpm or W: `from` and `to` are the lowest and the highest value in it. Z1 starts at 0; Z5 has no end. */
export interface Zone {
    name: string;
    from: number;
    to: number;
}

type Edges = readonly (readonly [percent: number, onEdgeUp: boolean])[];

// The web and the device must agree: these are the edges of `Metric::edges` in `firmware/obc-app/src/effort.rs`.
// The lower edges of Z2 to Z5 in percent of the limit. `true` puts a value exactly on the edge into the upper zone.
const HR_EDGES: Edges = [[60, true], [70, true], [80, true], [90, true]];
const POWER_EDGES: Edges = [[55, true], [75, false], [90, false], [105, false]];

function zones(limit: number, edges: Edges): Zone[] {
    const starts = [0, ...edges.map(([percent, up]) => up ? Math.ceil(percent * limit / 100) : Math.floor(percent * limit / 100) + 1)];
    return starts.map((from, i) => ({ name: `Z${i + 1}`, from, to: i + 1 < starts.length ? starts[i + 1] - 1 : Infinity }));
}

/** The index in `zones` of the zone of `value`: the last zone whose lowest value it reaches. */
export const zoneOf = (zones: Zone[], value: number): number => Math.max(0, zones.filter(zone => value >= zone.from).length - 1);

export const heartRateZones = (maxHr: number): Zone[] => zones(maxHr, HR_EDGES);
export const powerZones = (ftp: number): Zone[] => zones(ftp, POWER_EDGES);
