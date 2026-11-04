// The cross-section of a way from the ground plan's tables, as `Sections::way` of
// host/obc-ground-plan/src/sections.rs reads them, with `town = false`.
import CLASS_RULES from '../../../../../../data/trailview/roads.json' with { type: 'json' };
import FINISHES from '../../../../../../data/trailview/finishes.json' with { type: 'json' };
import { surfaceOf, trackSurface, type Surface } from './bins';

/** Metres, and rise per metre for `cut` and `fill`. */
export interface Section { finish: Surface; half_m: number; shoulder_m: number; cut: number; fill: number; reach_m: number }

/** As HALF_MIN_M and HALF_MAX_M of sections.rs. */
const HALF_MIN_M = 0.4, HALF_MAX_M = 15;
const TRACKS = ['rail', 'narrow_gauge', 'light_rail', 'funicular'];
const rows = CLASS_RULES as Record<string, { half_m: number; shoulder_m: number; cut: number; fill: number; reach: number }>;
const caps = FINISHES as Record<Surface, { half_m: number | null; shoulder_m: number | null }>;
const capped = (value: number, cap: number | null) => (cap === null ? value : Math.min(value, cap));

/** The section of a way of a feature kind; a class that the table lacks takes the row `path` for the kind `path`, else `road`. */
export function crossSection(kind: string, tags: Record<string, string>): Section {
    const track = TRACKS.includes(tags.railway) && !tags.highway;
    const name = track ? (tags.rack && tags.rack !== 'no' ? 'rack' : tags.railway) : (tags.highway ?? '');
    const row = Object.hasOwn(rows, name) ? rows[name] : rows[kind === 'path' ? 'path' : 'road'];
    const finish = track ? trackSurface(tags) : surfaceOf(tags, false), width = parseFloat(tags.width ?? '');
    const half = !track && Number.isFinite(width) ? Math.max(width / 2, HALF_MIN_M) : capped(row.half_m, caps[finish].half_m);
    return { finish, half_m: Math.min(half, HALF_MAX_M), shoulder_m: capped(row.shoulder_m, caps[finish].shoulder_m), cut: row.cut, fill: row.fill, reach_m: row.reach };
}
