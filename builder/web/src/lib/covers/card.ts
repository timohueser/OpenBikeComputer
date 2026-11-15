/** Draws the share card of a route or a ride: the picture or a plain ground with the line, the name, the numbers, the elevation profile, the date of a ride, the product mark and the credits of the map data. */
import type { Summary } from '../library/records';
import { drawCover, type CoverImage, type Look } from './draw';
import { metresAlong } from './line';

export type Format = 'wide';
/** The pixels of a card. The wide card has the size of a cover, so it shows the whole picture. */
export const SIZES: Record<Format, [number, number]> = { wide: [1920, 1280] };
/** "Hide start and end" leaves out HIDDEN_M of the line at each end. A line shorter than SHORT_M stays whole. */
export const HIDDEN_M = 500, SHORT_M = 2000;
/** The fonts of the card. A canvas draws a font only after the page loaded it. */
export const FONTS = ['700 96px Atkinson', '400 48px Atkinson', '700 30px "Atkinson Mono"'];

/** The card is the same in both looks. A plain ground has the colours of the light look; on a picture the text is white. */
const PLAIN = { ink: '#1c1b14', soft: '#5c5a2e', credit: '#676443', band: '#e7ecdf', profile: '#5c5a2e', post: '#5c5a2e' };
const ON_PICTURE = { ink: '#fff', soft: 'rgb(255 255 255 / 0.86)', credit: 'rgb(255 255 255 / 0.8)', band: 'rgb(255 255 255 / 0.34)', profile: 'rgb(255 255 255 / 0.92)', post: '#fff2d1' };
const GROUND = '#f4f2eb', BOARD = '#f4a81d', SHADE = '18 20 14';
/** `--route` and `--ride` of the light look. */
const LINE: Record<Look, string> = { route: '#cc2a93', ride: '#0b8f8b' };
/** The signpost of the product in a box of 100: the post and its two boards. */
const POST = [[44, 10], [56, 10], [56, 91], [44, 91]];
const BOARDS = [
    [[13.09, 26.42], [27.82, 16.42], [78.7, 19.98], [77.16, 41.93], [26.29, 38.37]],
    [[21.3, 53.98], [73.17, 50.35], [87.91, 60.35], [74.71, 72.3], [22.84, 75.93]],
];
const PRODUCT = 'OpenBikeComputer';
/** The profile band shows at least this range of heights, so a flat line looks flat. Chosen. */
const LEAST_RANGE_M = 50;
/** The name has at most NAME_LINES lines and the credits CREDIT_LINES; the rest ends with "…". */
const NAME_LINES = 3, CREDIT_LINES = 2;

export interface Card {
    name: string;
    look: Look;
    summary: Summary;
    /** The date of a ride. */
    date?: string;
    /** Each number with its unit, such as ["58.7", "km"]. With none, the card has no row of numbers. */
    numbers: [string, string][];
    /** The picture and the credits of its data. With none, the ground is plain. */
    cover?: CoverImage & { credits: string[] };
    line: boolean;
    profile: boolean;
    /** The metres of the line that the card leaves out at each end. */
    trim: number;
}

export interface Box { x: number; y: number; width: number; height: number }

/** The boxes of the parts of a card, in pixels. A part that the card does not show has no box. A box of text ends at
 * the baseline of its last line. */
export interface Layout {
    size: [number, number];
    /** The picture, or the plain ground. */
    picture: Box;
    /** The room of the line on a plain ground. */
    plan: Box;
    mark: Box;
    date?: Box;
    name: Box;
    numbers?: Box;
    credit?: Box;
    band?: Box;
    /** The sizes of the letters: the name, the numbers and the date, and the credits. */
    letters: { name: number; number: number; small: number };
}

/** The parts of a card that change its layout. The credits stand beside the numbers, or under them in their own lines. */
export interface Shown { nameLines: number; date: boolean; numbers: boolean; profile: boolean; creditLines: number; creditUnder: boolean }

/** The layout of a card: the picture fills it, the text stands at the lower left over the band of the profile, and the
 * mark at the upper left. */
export function layout(format: Format, shown: Shown): Layout {
    const [width, height] = SIZES[format];
    const margin = Math.round(0.042 * width), inner = width - 2 * margin;
    const letters = { name: Math.round(0.05 * width), number: Math.round(0.025 * width), small: Math.round(0.012 * width) };
    const nameLine = Math.round(1.08 * letters.name), smallLine = Math.round(1.4 * letters.small);
    const band = shown.profile ? { x: 0, y: height - Math.round(0.13 * height), width, height: Math.round(0.13 * height) } : undefined;
    // With no band, the text keeps the margin of the sides from the lower edge.
    const bottom = band ? band.y - Math.round(0.55 * margin) : height - margin;
    const creditHeight = shown.creditLines * smallLine;
    const credit = shown.creditLines ? { x: margin, y: bottom - creditHeight, width: inner, height: creditHeight } : undefined;
    const numbersBottom = credit && shown.creditUnder ? credit.y - Math.round(0.6 * letters.small) : bottom;
    const numbers = shown.numbers ? { x: margin, y: numbersBottom - letters.number, width: inner, height: letters.number } : undefined;
    const nameBottom = numbers ? numbersBottom - Math.round(1.45 * letters.number) : numbersBottom;
    const name = { x: margin, y: nameBottom - shown.nameLines * nameLine, width: inner, height: shown.nameLines * nameLine };
    const date = shown.date ? { x: margin, y: name.y - Math.round(0.25 * letters.number) - letters.number, width: inner, height: letters.number } : undefined;
    // The signpost, then the name of the product in mono letters of half its height, each 0.6 of its size wide.
    const markHeight = Math.round(0.62 * letters.name);
    const mark = { x: margin, y: Math.round(0.8 * margin), width: Math.round((1.05 + 0.3 * PRODUCT.length) * markHeight), height: markHeight };
    const planTop = Math.round(2.4 * margin), textTop = (date ?? name).y;
    return {
        size: [width, height], picture: { x: 0, y: 0, width, height }, mark, date, name, numbers, credit, band, letters,
        plan: { x: Math.round(1.5 * margin), y: planTop, width: width - 3 * margin, height: textTop - Math.round(0.9 * margin) - planTop },
    };
}

/** The credits of a release are sentences, too long for a card: the card names each holder of a right, the name after
 * each "©". Credits with no "©" stay as they are. */
export function holders(credits: string[]): string[] {
    const names = credits.flatMap(credit => [...credit.matchAll(/©\s*([^;,()/\d]+)/g)].map(match => `© ${match[1].trim()}`));
    return names.length ? [...new Set(names)] : credits;
}

/** The lines of `pieces` in the font of `context`: pieces joined by `joint`, broken between two pieces at `max` pixels.
 * The text after `most` lines, and the end of a piece wider than a line, is "…". */
function wrap(context: CanvasRenderingContext2D, pieces: string[], joint: string, max: number, most: number): string[] {
    const lines: string[] = [], wide = (text: string) => context.measureText(text).width > max;
    for (const piece of pieces) {
        const joined = `${lines.at(-1)}${joint}${piece}`;
        if (lines.length && !wide(joined)) lines[lines.length - 1] = joined;
        else lines.push(piece);
    }
    const kept = lines.slice(0, most);
    if (lines.length > most) kept[most - 1] += '…';
    return kept.map(line => {
        if (!wide(line)) return line;
        let start = line.replace(/…$/, '');
        while (start && wide(`${start}…`)) start = start.slice(0, -1);
        return `${start.trimEnd()}…`;
    });
}

/** The summary line from `trim` metres after its start to `trim` metres before its end, as [longitude, latitude]. An
 * end of the stretch can lie inside a segment. */
export function stretch(summary: Summary, trim: number): [number, number][] {
    const at = metresAlong(summary), [from, to] = [trim, summary.lengthM - trim], { line } = summary, points: [number, number][] = [];
    const place = (i: number, metres: number): [number, number] => {
        const t = Math.max(0, Math.min(1, (metres - at[i - 1]) / (at[i] - at[i - 1] || 1)));
        return [line[2 * i - 2] + t * (line[2 * i] - line[2 * i - 2]), line[2 * i - 1] + t * (line[2 * i + 1] - line[2 * i - 1])];
    };
    for (let i = 1; i < at.length; i++) {
        if (at[i] <= from || at[i - 1] >= to) continue;
        if (!points.length) points.push(place(i, from));
        points.push(place(i, to));
    }
    return points;
}

/** Draws the card over the whole canvas of `context`, which has the size of the format, and returns its layout. */
export function drawCard(context: CanvasRenderingContext2D, format: Format, card: Card): Layout {
    const { summary, cover } = card, [width, height] = SIZES[format], colours = cover ? ON_PICTURE : PLAIN;
    const sizes = layout(format, { nameLines: 1, date: false, numbers: false, profile: false, creditLines: 0, creditUnder: false });
    const { letters } = sizes, inner = sizes.name.width, gap = letters.number;
    const fonts = { name: `700 ${letters.name}px Atkinson`, value: `700 ${letters.number}px Atkinson`, unit: `400 ${letters.number}px Atkinson`, small: `400 ${letters.small}px Atkinson` };
    const measure = (font: string, text: string) => { context.font = font; return context.measureText(text).width; };

    context.font = fonts.name;
    const nameLines = wrap(context, card.name.split(/\s+/).filter(Boolean), ' ', inner, NAME_LINES);
    const numbers = card.numbers.map(([value, unit]) => ({ value, unit: unit && ` ${unit}`, width: measure(fonts.value, value) + (unit ? measure(fonts.unit, ` ${unit}`) : 0) }));
    const numbersWidth = numbers.reduce((sum, number) => sum + number.width, 0) + gap * Math.max(0, numbers.length - 1);
    // The credits stand at the right of the last line of the text, when they fit there in their lines.
    const beside = inner - gap - (numbers.length ? numbersWidth : measure(fonts.name, nameLines.at(-1) ?? ''));
    context.font = fonts.small;
    const names = cover ? holders(cover.credits) : [];
    let credits = wrap(context, names, ' · ', beside, Infinity);
    const creditUnder = credits.length > CREDIT_LINES || beside < inner / 4;
    if (creditUnder) credits = wrap(context, names, ' · ', inner, CREDIT_LINES);
    const boxes = layout(format, { nameLines: nameLines.length, date: card.date !== undefined, numbers: numbers.length > 0, profile: card.profile, creditLines: credits.length, creditUnder });

    if (cover) drawCover(context, cover, { box: [width, height], line: card.line, look: card.look, trim: card.trim });
    else {
        context.fillStyle = GROUND;
        context.fillRect(0, 0, width, height);
        if (card.line) plan(context, boxes.plan, stretch(summary, card.trim), LINE[card.look], Math.round(0.008 * Math.min(width, height)));
    }
    if (cover) {
        // A shade under the text and a weaker one under the mark, so that both read on snow and on a pale sky.
        const from = Math.max(0, (boxes.date ?? boxes.name).y - 0.22 * height), under = context.createLinearGradient(0, from, 0, height);
        under.addColorStop(0, `rgb(${SHADE} / 0)`);
        under.addColorStop(0.55, `rgb(${SHADE} / 0.62)`);
        under.addColorStop(1, `rgb(${SHADE} / 0.82)`);
        context.fillStyle = under;
        context.fillRect(0, from, width, height - from);
        const over = context.createLinearGradient(0, 0, 0, 0.16 * height);
        over.addColorStop(0, `rgb(${SHADE} / 0.42)`);
        over.addColorStop(1, `rgb(${SHADE} / 0)`);
        context.fillStyle = over;
        context.fillRect(0, 0, width, 0.16 * height);
    }

    if (boxes.band) profile(context, boxes.band, summary, colours.band, colours.profile);

    // Text on a picture has a tight shadow.
    if (cover) [context.shadowColor, context.shadowOffsetY, context.shadowBlur] = ['rgb(0 0 0 / 0.45)', 2, 4];
    context.textBaseline = 'alphabetic';
    const text = (font: string, colour: string, content: string, x: number, y: number) => {
        [context.font, context.fillStyle] = [font, colour];
        context.fillText(content, x, y);
        return context.measureText(content).width;
    };
    const base = (box: Box) => box.y + box.height;
    if (boxes.date) text(fonts.unit, colours.soft, card.date!, boxes.date.x, base(boxes.date));
    const nameLine = boxes.name.height / nameLines.length;
    nameLines.forEach((line, i) => text(fonts.name, colours.ink, line, boxes.name.x, boxes.name.y + (i + 1) * nameLine));
    let x = boxes.numbers?.x ?? 0;
    for (const { value, unit } of numbers) {
        x += text(fonts.value, colours.ink, value, x, base(boxes.numbers!));
        if (unit) x += text(fonts.unit, colours.soft, unit, x, base(boxes.numbers!));
        x += gap;
    }
    if (boxes.credit) {
        const smallLine = boxes.credit.height / credits.length;
        context.textAlign = creditUnder ? 'left' : 'right';
        credits.forEach((line, i) => text(fonts.small, colours.credit, line, boxes.credit!.x + (creditUnder ? 0 : boxes.credit!.width), boxes.credit!.y + (i + 1) * smallLine));
        context.textAlign = 'left';
    }

    const { mark } = boxes, scale = mark.height / 100;
    const shape = (points: number[][], colour: string) => {
        context.beginPath();
        points.forEach(([px, py], i) => context[i ? 'lineTo' : 'moveTo'](mark.x + px * scale, mark.y + py * scale));
        context.closePath();
        context.fillStyle = colour;
        context.fill();
    };
    shape(POST, colours.post);
    for (const board of BOARDS) shape(board, BOARD);
    context.textBaseline = 'middle';
    text(`700 ${Math.round(0.5 * mark.height)}px "Atkinson Mono"`, colours.ink, PRODUCT, mark.x + 1.05 * mark.height, mark.y + 0.52 * mark.height);
    context.textBaseline = 'alphabetic';
    [context.shadowColor, context.shadowOffsetY, context.shadowBlur] = ['transparent', 0, 0];
    return boxes;
}

/** The line from above with the north up, in the middle of `box`. */
function plan(context: CanvasRenderingContext2D, box: Box, points: [number, number][], colour: string, lineWidth: number): void {
    if (!points.length) return;
    const lons = points.map(point => point[0]), lats = points.map(point => point[1]);
    const [west, east, south, north] = [Math.min(...lons), Math.max(...lons), Math.min(...lats), Math.max(...lats)];
    const across = Math.cos((south + north) / 2 * Math.PI / 180);
    const scale = Math.min(box.width / ((east - west) * across || 1e-9), box.height / (north - south || 1e-9));
    context.beginPath();
    points.forEach(([lon, lat], i) => context[i ? 'lineTo' : 'moveTo'](
        box.x + box.width / 2 + (lon - (west + east) / 2) * across * scale, box.y + box.height / 2 - (lat - (south + north) / 2) * scale));
    context.lineCap = context.lineJoin = 'round';
    [context.lineWidth, context.strokeStyle] = [lineWidth, colour];
    context.stroke();
}

/** The heights over the distance, from edge to edge of `box`: a pale shape with a line on it. */
function profile(context: CanvasRenderingContext2D, box: Box, summary: Summary, fill: string, line: string): void {
    const at = metresAlong(summary), known = summary.heights.flatMap((height, i) => height === null ? [] : [[at[i] / (summary.lengthM || 1) * box.width, height]]);
    if (!known.length) return;
    const heights = known.map(point => point[1]), low = Math.min(...heights), range = Math.max(LEAST_RANGE_M, Math.max(...heights) - low);
    const floor = 0.16 * box.height, bottom = box.y + box.height;
    const y = (height: number) => bottom - floor - (height - low) / range * (box.height - floor);
    const ridge = [[box.x, y(heights[0])], ...known.map(([x, height]) => [box.x + x, y(height)]), [box.x + box.width, y(heights.at(-1)!)]];
    const trace = () => { context.beginPath(); ridge.forEach(([x, py], i) => context[i ? 'lineTo' : 'moveTo'](x, py)); };
    trace();
    context.lineTo(box.x + box.width, bottom);
    context.lineTo(box.x, bottom);
    context.closePath();
    context.fillStyle = fill;
    context.fill();
    trace();
    context.lineJoin = 'round';
    [context.lineWidth, context.strokeStyle] = [Math.max(2, 0.0016 * box.width), line];
    context.stroke();
}
