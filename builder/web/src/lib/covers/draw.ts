/** Draws a cover into a box: the image cut as CSS `cover` cuts it, the line of the route, a selected part and the mark. */
import type { CoverPoints, Summary } from '../library/records';
import { along, distanceAt, metresAlong, placed, runs } from './line';

/** The image is the same in both themes, so the line keeps the colours of the light theme: `--route` and `--amber`. */
const ROUTE = '#cc2a93', PART = '#f4a81d', CASING = 'rgba(255, 255, 255, 0.85)';
const HIDDEN_ALPHA = 0.3;
/** The width of the line at the point nearest to the eye, as a share of the shorter side of the box, and the least width as a share of it. */
const WIDEST = 0.012, LEAST = 1 / 3, CASING_PX = 1.5;

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
    const middle = (axis: Int16Array, size: number) => {
        let low = Infinity, high = -Infinity;
        for (let i = 0; i < axis.length; i++) if (placed(points, i)) [low, high] = [Math.min(low, axis[i]), Math.max(high, axis[i])];
        return low <= high ? (Math.max(low, 0) + Math.min(high, size)) / 2 : size / 2;
    };
    const place = (centre: number, size: number, shown: number) => Math.max(0, Math.min(size - shown, centre - shown / 2));
    return { scale, left: place(middle(points.x, cover.width), cover.width, width / scale), top: place(middle(points.y, cover.height), cover.height, height / scale) };
}

/** Draws the cover over the whole canvas of `context`, which is `box` pixels in size, and returns the cut. `mark` and `part` are metres along the route. */
export function drawCover(context: CanvasRenderingContext2D, cover: CoverImage, { box, mark, part, line }: { box: [number, number]; mark?: number; part?: [number, number]; line: boolean }): Crop {
    const cut = crop(cover, box), { points, summary } = cover;
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

    // The hidden stretches draw opaque on a layer of their own, which then draws weak: so their joints do not show as dots.
    const layer = new OffscreenCanvas(box[0], box[1]).getContext('2d')!;
    const all = runs(points);
    for (const [target, visible] of [[layer, false], [context, true]] as const) {
        target.lineCap = 'round';
        for (const pass of ['casing', 'colour'] as const) for (const run of all) {
            if (run.visible !== visible) continue;
            for (let i = run.from + 1; i <= run.to; i++) {
                target.beginPath();
                target.moveTo(x(i - 1), y(i - 1));
                target.lineTo(x(i), y(i));
                const w = (width(i - 1) + width(i)) / 2;
                target.lineWidth = pass === 'casing' ? w + 2 * CASING_PX : w;
                target.strokeStyle = pass === 'casing' ? CASING : inPart(i) ? PART : ROUTE;
                target.stroke();
            }
        }
        if (!visible) {
            context.globalAlpha = HIDDEN_ALPHA;
            context.drawImage(layer.canvas, 0, 0);
            context.globalAlpha = 1;
        }
    }
    const at = summary && mark !== undefined ? along(points, summary, mark) : undefined;
    if (at) {
        context.beginPath();
        context.arc((at.x - cut.left) * cut.scale, (at.y - cut.top) * cut.scale, widest * 1.2, 0, 2 * Math.PI);
        context.fillStyle = '#fff';
        context.fill();
        context.lineWidth = widest * 0.5;
        context.strokeStyle = ROUTE;
        context.stroke();
    }
    return cut;
}
