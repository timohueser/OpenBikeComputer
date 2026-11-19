/** The page of a long trail: its address, a row for each stage, the profile of the whole trail with its stages, and the
 * cell files and the box of the stage lines on its map. */
import type { Tick } from '../../components/route/ticks';
import type { Bounds } from '../../lib/planner/config';
import { duration } from '../../lib/planner/editor';
import { kmPerDegree, type Coordinate } from '../../lib/planner/geo';
import { coversCell } from '../../lib/planner/map-data';
import type { ProfileLine } from '../../lib/planner/profile-data';
import type { Box } from '../../lib/planner/route-catalog';
import { gradeName, type CatalogRecord, type TrailPart } from '../../lib/planner/signed-routes';
import { ROUTES } from '../../lib/routes';
import { formatDistance, formatHeight } from '../../lib/settings/format';
import { partLabel, partSection, signedHref } from './signed-page';

export interface TrailRow {
    id: number;
    label: string | false;
    numbers: string[];
    /** The page of the stage; a gap has none. */
    href?: string;
    /** A part inside the area of the release that the catalog does not hold. */
    gap: boolean;
}

/** The least width of a stage on the profile for its number. */
const NUMBER_PX = 18;

export const trailHref = (id: number) => `${ROUTES.trail}/${id}`;

/** The last `end_m`: the length of the axis of the profile, in metres. */
export const lastEnd = (parts: TrailPart[]) => parts.at(-1)?.end_m ?? 0;

/** A row for each part in the catalog and for each gap, in trail order. A part outside the area of the release, with no
 * ends, has no row. A row shows the length, the climb, the time and the grade that the part has. */
export function trailRows(parts: TrailPart[], kind: CatalogRecord['kind'], bounds: Bounds): TrailRow[] {
    return parts.filter(part => !part.missing || part.start_udeg).map(part => {
        const gap = !!part.missing;
        const numbers = [
            part.length_m !== undefined && formatDistance(part.length_m),
            gap ? 'Not in the map data' : part.ascent_m !== undefined && `${formatHeight(part.ascent_m)} up`,
            !gap && part.moving_s !== undefined && duration(part.moving_s / 3600),
            !gap && kind !== 'bicycle' && part.hardest !== undefined && gradeName(kind, part.hardest),
        ].filter((number): number is string => !!number);
        const href = gap || !part.cells ? undefined : signedHref({ id: part.id, cells: part.cells }, bounds);
        return { id: part.id, label: partLabel(part.stage, partSection(part), part.name), numbers, ...href ? { href } : {}, gap };
    });
}

/** The profile of the whole trail as a line along the equator, so that its length in kilometres is the last `end_m`: the
 * heights of `trail_profile_m` stand at equal shares of it. */
export function trailProfileLine(record: CatalogRecord): ProfileLine {
    const heights = record.trail_profile_m ?? [], degrees = lastEnd(record.parts ?? []) / 1000 / kmPerDegree;
    return { coordinates: heights.map((_, k) => [k / Math.max(1, heights.length - 1) * degrees, 0]), elevation: heights };
}

/** The stretch of the axis of each part with a length, as shares of the last `end_m`. */
export function stageStretches(parts: TrailPart[]): { id: number; stage?: number; from: number; to: number }[] {
    const total = lastEnd(parts);
    return parts.flatMap((part, i) => {
        const start = parts[i - 1]?.end_m ?? 0;
        return part.end_m > start ? [{ id: part.id, ...part.stage === undefined ? {} : { stage: part.stage }, from: start / total, to: part.end_m / total }] : [];
    });
}

/** The number of each stage at the middle of its stretch, the sections of a stage at the middle of their joint stretch,
 * when the stretch of each numbered stage is at least `NUMBER_PX` wide on an axis `width` px wide; else none. */
export function stageNumbers(stretches: ReturnType<typeof stageStretches>, width: number): Tick[] {
    const stages = new Map<number, [number, number]>();
    for (const { stage, from, to } of stretches) {
        if (stage === undefined) continue;
        const [a, b] = stages.get(stage) ?? [from, to];
        stages.set(stage, [Math.min(a, from), Math.max(b, to)]);
    }
    const spans = [...stages];
    return spans.every(([, [from, to]]) => (to - from) * width >= NUMBER_PX)
        ? spans.map(([stage, [from, to]]) => ({ at: (from + to) / 2, label: String(stage) })) : [];
}

/** The ends of the parts inside the axis, each once, as shares of the last `end_m`. */
export function stageBorders(parts: TrailPart[]): number[] {
    const total = lastEnd(parts);
    return [...new Set(parts.map(part => part.end_m))].filter(end => end > 0 && end < total).map(end => end / total);
}

/** Where each part in the catalog but the first starts in trail order: the first point of its line, or the last when its
 * line runs against the trail. A part with no line has no point. */
export function borderPoints(parts: TrailPart[], lineOf: (id: number) => Coordinate[] | undefined): Coordinate[] {
    return parts.filter(part => !part.missing).slice(1).flatMap(part => {
        const line = lineOf(part.id), point = part.backwards ? line?.at(-1) : line?.[0];
        return point ? [point] : [];
    });
}

/** The cell files that give the lines of the parts in the catalog whose records `recordOf` does not hold: in trail order,
 * a part takes a covered cell that an earlier part took, else its first covered cell. */
export function stageCells(parts: TrailPart[], recordOf: (id: number) => CatalogRecord | undefined, bounds: Bounds): string[] {
    const cells: string[] = [];
    for (const part of parts) {
        if (part.missing || recordOf(part.id)) continue;
        const covered = (part.cells ?? []).filter(cell => coversCell(bounds, cell));
        const cell = covered.find(one => cells.includes(one)) ?? covered[0];
        if (cell !== undefined && !cells.includes(cell)) cells.push(cell);
    }
    return cells;
}

/** The box of the points of `lines`. */
export function lineBox(lines: Coordinate[][]): Box {
    let [west, south, east, north] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const line of lines) {
        for (const [lon, lat] of line) {
            [west, east] = [Math.min(west, lon), Math.max(east, lon)];
            [south, north] = [Math.min(south, lat), Math.max(north, lat)];
        }
    }
    return [west, south, east, north];
}
