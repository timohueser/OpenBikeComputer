/** The address of the page of a signed route, the Explore state that leads back to it, the description it shows, and the
 * names of the parts of its trail. */
import type { Bounds } from '../../lib/planner/config';
import { coversCell } from '../../lib/planner/map-data';
import type { Box } from '../../lib/planner/route-catalog';
import { decodeCoordinates } from '../../lib/planner/route-answer';
import { longTrailId, type CatalogRecord, type TrailPart } from '../../lib/planner/signed-routes';
import { ROUTES } from '../../lib/routes';
import type { Chips } from '../routes/filter';

/** `#/signed/<cell>/<id>` with the first cell of the record that the release covers: `cells` can name a cell outside the
 * grid, which has no file. Undefined when the release covers none. */
export function signedHref(record: Pick<CatalogRecord, 'id' | 'cells'>, bounds: Bounds): string | undefined {
    const cell = record.cells.find(cell => coversCell(bounds, cell));
    return cell && `${ROUTES.signed}/${cell}/${record.id}`;
}

/** The cell, its zoom and its box, and the record id of the address `<cell>/<id>` of the page; undefined for another form. */
export function readAddress(address: string): { cell: string; zoom: number; box: Box; id: number } | undefined {
    const match = /^((\d{1,2})-(\d+)-(\d+))\/(-?\d+)$/.exec(address);
    if (!match) return undefined;
    const [zoom, x, y] = [2, 3, 4].map(i => Number(match[i])), n = 2 ** zoom;
    if (x >= n || y >= n) return undefined;
    const latitude = (row: number) => Math.atan(Math.sinh(Math.PI * (1 - 2 * row / n))) * 180 / Math.PI;
    return { cell: match[1], zoom, box: [x / n * 360 - 180, latitude(y + 1), (x + 1) / n * 360 - 180, latitude(y)], id: Number(match[5]) };
}

/** The Explore state that shows `record`: the view of its line, the bike of its kind and its id. Explore decides the tab and
 * the card. */
export function exploreAt(record: CatalogRecord): { chips: Chips; view?: Box; show: number } {
    const bike = record.kind === 'mtb' ? 'mtb' : record.kind === 'bicycle' ? 'touring' : 'hiking';
    const points = decodeCoordinates(record.line_udeg ?? []);
    const lons = points.map(point => point[0]), lats = points.map(point => point[1]);
    const view: Box | undefined = points.length ? [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)] : undefined;
    return { chips: { bike }, ...view ? { view } : {}, show: record.id };
}

/** Words that name a part of a route, in the languages of the catalog. */
const STAGE_WORDS = new Set(['etappe', 'stage', 'étape', 'etape', 'tappa', 'section', 'abschnitt', 'teil', 'part', 'day', 'tag', 'variante', 'variant']);
const TOKEN = /[^\s\-–—»>/:,;.!?()&+'’]+/gu;
const LETTERS = /[\p{L}\p{M}]+/gu;
/** A dash, an arrow or » between two words, or a hyphen from a small letter to a capital letter. */
const DASH = /\s-\s|[–—»]|->|(?<=\p{Ll})-(?=\p{Lu})/u;

/**
 * The OSM description of a record when it says something that the record does not: at least 3 new words and no list of
 * places. A word is a run of letters with no digit and no underscore; it is new when the name, the ref or the operator of
 * the record or of its long route does not hold it, and it is no stage word. A list of places has a dash between two words
 * and no new word of more than 3 letters that starts with a small letter.
 */
export function shownDescription(record: CatalogRecord, recordOf: (id: number) => CatalogRecord | undefined): string | undefined {
    const text = record.description;
    if (!text) return undefined;
    const longId = record.trail ?? record.parent, long = longId === undefined ? undefined : recordOf(longId);
    const own = new Set([record, long].flatMap(source => [source?.name, source?.ref, source?.operator])
        .flatMap(field => field?.toLowerCase().match(LETTERS) ?? []));
    const fresh = (text.match(TOKEN) ?? []).map(token => token.replace(/^"+|"+$/g, ''))
        .filter(token => /\p{L}/u.test(token) && !/[\d_]/.test(token))
        .filter(token => !own.has(token.toLowerCase()) && !STAGE_WORDS.has(token.toLowerCase()));
    const places = DASH.test(text) && !fresh.some(token => token.length > 3 && /^\p{Ll}/u.test(token));
    return fresh.length >= 3 && !places ? text : undefined;
}

/**
 * The place of a stage or a section in its long route, "Stage 2 of 5" or "Section 3", that long route, and the trail that
 * its name opens. The long route is `trail ?? parent`, but a stage whose `trail` has fewer than two stages is named in
 * `parent`, the complete long route that holds it (`byParent`). The card and the page of the route use this one rule, so
 * they need no far file. `parent` and `trail` can name two long routes: the number is of the one named.
 */
export function stagePart(record: CatalogRecord, recordOf: (id: number) => CatalogRecord | undefined) {
    const inner = record.trail === undefined ? undefined : recordOf(record.trail);
    const byParent = record.parent !== undefined && record.trail !== undefined && record.trail !== record.parent
        && (inner?.stages?.length ?? inner?.stage_count ?? 0) < 2;
    const longId = byParent ? record.parent : record.trail ?? record.parent, long = longId === undefined ? undefined : recordOf(longId);
    const number = record.trail !== undefined && !byParent ? record.trail_stage : record.stage, of = long?.stages?.length ?? long?.stage_count;
    const part: string | false = record.section !== undefined ? `Section ${record.section}` : number === undefined ? false : of ? `Stage ${number} of ${of}` : `Stage ${number}`;
    return { part, long, byParent, trailId: byParent ? record.parent : longTrailId(record, recordOf) };
}

/** The number of a section in its cut line: a section has it in its id, −(relation × 100 + section). */
export const partSection = (part: Pick<TrailPart, 'id'>) => part.id < 0 ? -part.id % 100 : undefined;

/** The name of a part of a trail: "Stage 8 · Section 2", "Section 2", "Stage 8", else `name`. */
export function partLabel(stage?: number, section?: number, name?: string): string | false {
    const words = [stage !== undefined && `Stage ${stage}`, section !== undefined && `Section ${section}`].filter(Boolean);
    return words.join(' · ') || name || false;
}
