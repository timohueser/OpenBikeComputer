/** Draws the share card of a route or a ride: the whole picture, or a plain ground with the line, at the top with the product mark, and under it the date of a ride, the name, the numbers and the elevation profile on a plain ground. */
import type { Summary } from '../library/records';
import { drawCover, type CoverImage, type Look } from './draw';
import { fromAbove, metresAlong } from './line';

export type Format = 'wide' | 'square' | 'tall';
/** The size of a card in pixels. The wide card shows a cover at its own size, and is as high as the picture and the
 * ground of its name need. */
const SIZES: Record<Format, { width: number; height?: number }> = { wide: { width: 1920 }, square: { width: 1280, height: 1280 }, tall: { width: 1024, height: 1280 } };
/** "Hide start and end" leaves out HIDDEN_M of the line at each end. A line shorter than SHORT_M stays whole. */
export const HIDDEN_M = 500, SHORT_M = 2000;
/** The fonts of the card. A canvas draws a font only after the page loaded it. */
export const FONTS = ['700 96px Atkinson', '400 48px Atkinson', '700 30px "Atkinson Mono"'];
/** A missing font file must not stop the card: each font names the fallback of the product. */
const SANS = 'Atkinson, system-ui, sans-serif', MONO = '"Atkinson Mono", ui-monospace, monospace';

/** The card is the same in both looks: the ground and the text have the colours of the light look. */
const GROUND = '#f4f2eb', INK = '#1c1b14', SOFT = '#5c5a2e', FAINT = '#676443', BAND = '#e7ecdf', BOARD = '#f4a81d';
/** The mark on a plain ground, and on a picture. */
const MARK = { plain: { text: INK, post: SOFT }, picture: { text: '#fff', post: '#fff2d1' } };
/** `--route` and `--ride` of the light look. */
const LINE: Record<Look, string> = { route: '#cc2a93', ride: '#0b8f8b' };
/** The signpost of the product in a box of 100: the post and its two boards. */
const POST = [[44, 10], [56, 10], [56, 91], [44, 91]];
const BOARDS = [
    [[13.09, 26.42], [27.82, 16.42], [78.7, 19.98], [77.16, 41.93], [26.29, 38.37]],
    [[21.3, 53.98], [73.17, 50.35], [87.91, 60.35], [74.71, 72.3], [22.84, 75.93]],
];
const PRODUCT = 'OpenBikeComputer';
/** Each card with a picture has this one credit line. */
const CREDIT = '© OpenStreetMap contributors and others';
/** The profile shows at least this range of heights, so a flat line looks flat. Chosen. */
const LEAST_RANGE_M = 50;
/** The name has at most NAME_LINES lines; the rest ends with "…". */
const NAME_LINES = 3;
/** The height of a capital letter, as a share of the size of its letters. */
const CAPITAL = 0.7;

export interface Card {
    name: string;
    look: Look;
    summary: Summary;
    /** The date of a ride. */
    date?: string;
    /** Each number with its unit, such as ["58.7", "km"]. With none, the card has no row of numbers. */
    numbers: [string, string][];
    /** The picture. With none, the top of the card is a plain ground. */
    cover?: CoverImage;
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
    /** The whole picture at the top, 3 : 2 as a cover; or a plain ground. Only the mark stands on it. */
    picture: Box;
    /** The room of the line on a plain ground, in the place of the picture. */
    plan: Box;
    mark: Box;
    /** The plain ground under the picture, with the text and the profile. */
    ground: Box;
    date?: Box;
    name: Box;
    numbers?: Box;
    credit?: Box;
    /** The elevation profile, along the lower edge of the ground. */
    band?: Box;
    /** The sizes of the letters: the name, the numbers and the date, and the credit. */
    letters: { name: number; number: number; small: number };
}

/** The parts of a card that change its layout. */
export interface Shown { nameLines: number; date: boolean; numbers: boolean; profile: boolean; credit: boolean }

/** The layout of a card. Each size is a share of the width, so the text of each format reads the same on a screen
 * of one width. The picture is whole, 3 : 2, and the ground takes the rest of the card. The ground of the wide card
 * is as high as the text of a name of one line with each of its rows and a profile need (0.26 of the width, chosen),
 * and one line higher for each more line of the name: no switch changes the size of a card.
 *
 * The profile takes the height that the text at the margin leaves, up to 0.4 of the ground; a profile lower than 0.03
 * of the width is no profile. The text stands in the middle of the room over the profile, at least the margin under
 * the picture. */
export function layout(format: Format, shown: Shown): Layout {
    const { width, height } = SIZES[format], margin = Math.round(0.042 * width), inner = width - 2 * margin;
    const letters = { name: Math.round(0.05 * width), number: Math.round(0.025 * width), small: Math.round(0.012 * width) };
    const nameLine = Math.round(1.08 * letters.name);
    const picture = { x: 0, y: 0, width, height: Math.round(width * 2 / 3) };
    const bottom = height ?? picture.height + Math.round(0.2604 * width) + (shown.nameLines - 1) * nameLine;
    const ground = { x: 0, y: picture.height, width, height: bottom - picture.height };
    // The baseline of each row, from the top of the letters of the first row.
    const dateBase = shown.date ? Math.round(CAPITAL * letters.number) : undefined;
    const nameBase = dateBase === undefined ? Math.round(CAPITAL * letters.name) : dateBase + Math.round(0.25 * letters.number) + nameLine;
    const nameEnd = nameBase + (shown.nameLines - 1) * nameLine;
    const numbersBase = shown.numbers ? nameEnd + Math.round(1.45 * letters.number) : undefined;
    const creditBase = shown.credit ? (numbersBase ?? nameEnd) + Math.round(1.9 * letters.small) : undefined;
    const end = creditBase ?? numbersBase ?? nameEnd;
    const room = Math.min(Math.round(0.4 * ground.height), ground.height - margin - end - Math.round(0.55 * margin));
    const band = shown.profile && room >= Math.round(0.03 * width) ? { x: 0, y: bottom - room, width, height: room } : undefined;
    const top = ground.y + Math.max(margin, Math.round((ground.height - (band?.height ?? 0) - end) / 2));
    const row = (base: number | undefined, size: number) => base === undefined ? undefined : { x: margin, y: top + base - size, width: inner, height: size };
    // The signpost, then the name of the product in mono letters of half its height, each 0.6 of its size wide.
    const markHeight = Math.round(0.62 * letters.name), planTop = Math.round(2.4 * margin);
    return {
        size: [width, bottom], picture, ground, letters, band,
        mark: { x: margin, y: Math.round(0.8 * margin), width: Math.round((1.05 + 0.3 * PRODUCT.length) * markHeight), height: markHeight },
        plan: { x: Math.round(1.5 * margin), y: planTop, width: width - 3 * margin, height: picture.height - planTop - margin },
        date: row(dateBase, letters.number),
        name: { x: margin, y: top + nameBase - nameLine, width: inner, height: shown.nameLines * nameLine },
        numbers: row(numbersBase, letters.number),
        credit: row(creditBase, letters.small),
    };
}

/** The lines of `text` in the font of `context`, broken at spaces to `max` pixels. The text after `most` lines, and
 * the end of a word wider than a line, is "…". */
function wrap(context: CanvasRenderingContext2D, text: string, max: number, most: number): string[] {
    const lines: string[] = [], wide = (line: string) => context.measureText(line).width > max;
    for (const word of text.split(/\s+/).filter(Boolean)) {
        const joined = `${lines.at(-1)} ${word}`;
        if (lines.length && !wide(joined)) lines[lines.length - 1] = joined;
        else lines.push(word);
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

/** Gives `canvas` the size of the card, draws the card over it and returns its layout. */
export function drawCard(canvas: HTMLCanvasElement, format: Format, card: Card): Layout {
    const { summary, cover } = card, context = canvas.getContext('2d')!;
    const { letters, name } = layout(format, { nameLines: 1, date: false, numbers: false, profile: false, credit: false });
    const fonts = { name: `700 ${letters.name}px ${SANS}`, value: `700 ${letters.number}px ${SANS}`, unit: `400 ${letters.number}px ${SANS}`, small: `400 ${letters.small}px ${SANS}` };
    context.font = fonts.name;
    const nameLines = wrap(context, card.name, name.width, NAME_LINES);
    const boxes = layout(format, { nameLines: nameLines.length, date: card.date !== undefined, numbers: card.numbers.length > 0, profile: card.profile, credit: !!cover });
    const { picture, mark } = boxes;
    // A new size clears the canvas and its settings.
    [canvas.width, canvas.height] = boxes.size;

    context.fillStyle = GROUND;
    context.fillRect(0, 0, ...boxes.size);
    if (cover) {
        // The glow of a line at the lower edge of the picture stays in the picture.
        context.save();
        context.beginPath();
        context.rect(picture.x, picture.y, picture.width, picture.height);
        context.clip();
        drawCover(context, cover, { box: [picture.width, picture.height], line: card.line, look: card.look, trim: card.trim });
        // A weak shade under the mark, so that it reads on snow and on a pale sky.
        const shade = context.createLinearGradient(0, 0, 0, 0.16 * picture.height);
        shade.addColorStop(0, 'rgb(18 20 14 / 0.42)');
        shade.addColorStop(1, 'rgb(18 20 14 / 0)');
        context.fillStyle = shade;
        context.fillRect(0, 0, picture.width, 0.16 * picture.height);
        context.restore();
    } else if (card.line) plan(context, boxes.plan, stretch(summary, card.trim), LINE[card.look], Math.round(0.005 * picture.width));
    if (boxes.band) profile(context, boxes.band, summary);

    context.textBaseline = 'alphabetic';
    const text = (font: string, colour: string, content: string, x: number, y: number) => {
        [context.font, context.fillStyle] = [font, colour];
        context.fillText(content, x, y);
        return context.measureText(content).width;
    };
    const base = (box: Box) => box.y + box.height;
    if (boxes.date) text(fonts.unit, SOFT, card.date!, boxes.date.x, base(boxes.date));
    const nameLine = boxes.name.height / nameLines.length;
    nameLines.forEach((line, i) => text(fonts.name, INK, line, boxes.name.x, boxes.name.y + (i + 1) * nameLine));
    let x = boxes.numbers?.x ?? 0;
    for (const [value, unit] of card.numbers) {
        x += text(fonts.value, INK, value, x, base(boxes.numbers!));
        if (unit) x += text(fonts.unit, SOFT, ` ${unit}`, x, base(boxes.numbers!));
        x += letters.number;
    }
    if (boxes.credit) text(fonts.small, FAINT, CREDIT, boxes.credit.x, base(boxes.credit));

    const colours = cover ? MARK.picture : MARK.plain, scale = mark.height / 100;
    // The mark on a picture has a tight shadow.
    if (cover) [context.shadowColor, context.shadowOffsetY, context.shadowBlur] = ['rgb(0 0 0 / 0.45)', 2, 4];
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
    text(`700 ${Math.round(0.5 * mark.height)}px ${MONO}`, colours.text, PRODUCT, mark.x + 1.05 * mark.height, mark.y + 0.52 * mark.height);
    return boxes;
}

/** The line from above with the north up, in the middle of `box`. */
function plan(context: CanvasRenderingContext2D, box: Box, line: [number, number][], colour: string, lineWidth: number): void {
    if (!line.length) return;
    const { points, width, height } = fromAbove(line), scale = Math.min(box.width / (width || 1e-9), box.height / (height || 1e-9));
    const left = box.x + (box.width - width * scale) / 2, top = box.y + (box.height - height * scale) / 2;
    context.beginPath();
    points.forEach(([x, y], i) => context[i ? 'lineTo' : 'moveTo'](left + x * scale, top + y * scale));
    context.lineCap = context.lineJoin = 'round';
    [context.lineWidth, context.strokeStyle] = [lineWidth, colour];
    context.stroke();
}

/** The heights over the distance, from edge to edge of `box`: a pale shape with a line on it. */
function profile(context: CanvasRenderingContext2D, box: Box, summary: Summary): void {
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
    context.fillStyle = BAND;
    context.fill();
    trace();
    context.lineJoin = 'round';
    [context.lineWidth, context.strokeStyle] = [Math.max(2, 0.0016 * box.width), SOFT];
    context.stroke();
}
