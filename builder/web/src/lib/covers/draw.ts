/** Draws a cover into a box: the image cut as CSS `cover` cuts it, the line of the route, a selected part and the mark. */
import type { CoverPoints, Summary } from '../library/records';
import { along, distanceAt, metresAlong, placed, runs } from './line';

/** The image is the same in both themes, so the line of a route keeps the colours of the light theme: `--route`, and
 * `--amber` for a part. As the ribbon of TrailView, a glow in a lighter tint of the line fades out beyond it, and a weak
 * light core runs in its middle. The line of a ride is warm white with an amber glow; `ring` is the edge of the mark. */
const LOOKS = {
    route: { line: '#cc2a93', glow: '#ee8fcf', core: '#f8b8e2', ring: '#cc2a93' },
    ride: { line: '#fff3df', glow: '#f2a33a', core: '#ffffff', ring: '#b4531b' },
    part: { line: '#f4a81d', glow: '#f8c66a', core: '#ffe6b0' },
};
export type Look = 'route' | 'ride';
/** With a part, the rest of the line takes a paler solid tint of its look. */
export const REST: Record<Look, { line: string; glow: string; core: string }> = {
    route: { line: '#e7a6d0', glow: '#f2cde4', core: '#fae6f2' },
    ride: { line: '#d9d0c4', glow: '#e3d6c3', core: '#f2ece4' },
};
/** The glow reaches GLOW_REACH of the widest line beyond it, in GLOW_STEPS strokes of GLOW_ALPHA each; the core is CORE_WIDTH of the line. */
const GLOW_REACH = 0.6, GLOW_STEPS = 5, GLOW_ALPHA = 0.14, CORE_WIDTH = 0.25, CORE_ALPHA = 0.3;
/** The width of the line at the point nearest to the eye, as a share of the shorter side of the box, and the least width as a share of it. */
const WIDEST = 0.012, LEAST = 1 / 3;

/** The cut of the image: canvas pixel = (image pixel − `left`, `top`) · `scale`. */
export interface Crop { scale: number; left: number; top: number }

export interface CoverImage {
    image: CanvasImageSource;
    width: number;
    height: number;
    points: CoverPoints;
    /** The summary that the points belong to. Without it, the line shows with no part and no mark. */
    summary?: Summary;
}

/** The cut for a box of `width` × `height`: it fills the box as CSS `cover` does, and moves toward the middle of the route as far as the image permits. */
export function crop(cover: Pick<CoverImage, 'width' | 'height' | 'points'>, [width, height]: [number, number]): Crop {
    const scale = Math.max(width / cover.width, height / cover.height), { points } = cover;
    const middle = (axis: Float32Array, size: number) => {
        let low = Infinity, high = -Infinity;
        for (let i = 0; i < axis.length; i++) if (placed(points, i)) [low, high] = [Math.min(low, axis[i]), Math.max(high, axis[i])];
        return low <= high ? (Math.max(low, 0) + Math.min(high, size)) / 2 : size / 2;
    };
    const place = (centre: number, size: number, shown: number) => Math.max(0, Math.min(size - shown, centre - shown / 2));
    return { scale, left: place(middle(points.x, cover.width), cover.width, width / scale), top: place(middle(points.y, cover.height), cover.height, height / scale) };
}

/** Draws the cover over the whole canvas of `context`, which is `box` pixels in size, and returns the cut. `mark` and `part` are metres along the route. */
export function drawCover(context: CanvasRenderingContext2D, cover: CoverImage,
    { box, mark, part, line, look = 'route' }: { box: [number, number]; mark?: number; part?: [number, number]; line: boolean; look?: Look }): Crop {
    const cut = crop(cover, box), { points, summary } = cover, colours = LOOKS[look];
    context.drawImage(cover.image, cut.left, cut.top, box[0] / cut.scale, box[1] / cut.scale, 0, 0, box[0], box[1]);
    if (!line) return cut;
    const x = (i: number) => (points.x[i] - cut.left) * cut.scale, y = (i: number) => (points.y[i] - cut.top) * cut.scale;
    // The width falls with the distance from the eye.
    let nearest = Infinity;
    for (let i = 0; i < points.x.length; i++) if (placed(points, i)) nearest = Math.min(nearest, distanceAt(points, i));
    const widest = Math.max(2, WIDEST * Math.min(...box));
    const width = (i: number) => widest * Math.max(LEAST, Math.min(1, nearest / distanceAt(points, i)));
    const metres = summary && part && metresAlong(summary);
    const inPart = (i: number) => !!metres && !!part && (metres[i - 1] + metres[i]) / 2 >= part[0] && (metres[i - 1] + metres[i]) / 2 <= part[1];

    // Consecutive segments of one width step and one colour make one path, so that a weak stroke shows no dots at its
    // joints. A stretch that the terrain hides draws nothing.
    const pieces: { part: boolean; w: number; from: number; to: number }[] = [];
    for (const run of runs(points)) for (let i = run.from + 1; run.visible && i <= run.to; i++) {
        const w = Math.round(width(i - 1) + width(i)) / 2, last = pieces.at(-1), inside = inPart(i);
        if (last?.to === i - 1 && last.w === w && last.part === inside) last.to = i;
        else pieces.push({ part: inside, w, from: i - 1, to: i });
    }
    const reach = Math.max(2, GLOW_REACH * widest), glow = Array.from({ length: GLOW_STEPS }, (_, k) => 2 * reach * (GLOW_STEPS - k) / GLOW_STEPS);
    const tint = (inside: boolean) => inside ? LOOKS.part : metres ? REST[look] : colours;
    const passes: { width: (w: number) => number; colour: (inside: boolean) => string; alpha: number }[] = [
        ...glow.map(extra => ({ width: (w: number) => w + extra, colour: (inside: boolean) => tint(inside).glow, alpha: GLOW_ALPHA })),
        { width: w => w, colour: inside => tint(inside).line, alpha: 1 },
        { width: w => CORE_WIDTH * w, colour: inside => tint(inside).core, alpha: CORE_ALPHA },
    ];
    context.lineCap = context.lineJoin = 'round';
    for (const pass of passes) {
        context.globalAlpha = pass.alpha;
        for (const piece of pieces) {
            context.beginPath();
            context.moveTo(x(piece.from), y(piece.from));
            for (let i = piece.from + 1; i <= piece.to; i++) context.lineTo(x(i), y(i));
            context.lineWidth = pass.width(piece.w);
            context.strokeStyle = pass.colour(piece.part);
            context.stroke();
        }
    }
    context.globalAlpha = 1;
    const at = summary && mark !== undefined ? along(points, summary, mark) : undefined;
    if (at) {
        const cx = (at.x - cut.left) * cut.scale, cy = (at.y - cut.top) * cut.scale, r = 1.2 * widest;
        context.fillStyle = colours.glow;
        context.globalAlpha = GLOW_ALPHA;
        for (const extra of glow) {
            context.beginPath();
            context.arc(cx, cy, r + extra / 2, 0, 2 * Math.PI);
            context.fill();
        }
        context.globalAlpha = 1;
        context.beginPath();
        context.arc(cx, cy, r, 0, 2 * Math.PI);
        context.fillStyle = '#fff';
        context.fill();
        context.lineWidth = widest * 0.5;
        context.strokeStyle = colours.ring;
        context.stroke();
    }
    return cut;
}
