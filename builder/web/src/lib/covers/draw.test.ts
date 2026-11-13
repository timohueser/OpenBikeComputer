import { describe, expect, it } from 'vitest';
import type { Summary } from '../library/records';
import { drawCover, REST } from './draw';
import { pack } from './line';

// Five visible points 100 m and 100 px apart along y = 50.
const x = [0, 100, 200, 300, 400];
const points = pack({
    x: Float32Array.from(x), y: Float32Array.from(x, () => 50),
    distance: Float32Array.from(x, (_, i) => 1000 * (i + 1)), visible: Uint8Array.from(x, () => 1),
});
const summary = {
    line: x.flatMap((_, i) => [i * 0.0009, 0]), heights: x.map(() => null), bounds: [0, 0, 0.0036, 0],
    lengthM: 400, climbM: 0, highestM: null, seconds: 0, bike: 'road',
} satisfies Summary;

/** A context that keeps the colour and the strength of each stroke, and the x where its path starts. */
function recorder() {
    const strokes: { colour: string; alpha: number; from: number }[] = [];
    let from = 0;
    const context = {
        globalAlpha: 1, strokeStyle: '', fillStyle: '', lineWidth: 1, lineCap: '', lineJoin: '',
        drawImage() {}, beginPath() {}, moveTo(x: number) { from = x; }, lineTo() {}, arc() {}, fill() {},
        stroke() { strokes.push({ colour: context.strokeStyle, alpha: context.globalAlpha, from }); },
    };
    return { context: context as unknown as CanvasRenderingContext2D, strokes };
}
const LINE = '#cc2a93', GLOW = '#ee8fcf', CORE = '#f8b8e2';
const cover = { image: {} as CanvasImageSource, width: 400, height: 100, points, summary };
function strokesOf(part?: [number, number]) {
    const { context, strokes } = recorder();
    drawCover(context, cover, { box: [400, 100], line: true, part });
    return strokes;
}

describe('the line of a cover', () => {
    it('draws no pale colour with no part', () => {
        expect(strokesOf().map(stroke => stroke.colour)).not.toContain(REST.route);
    });

    it('draws the part in the colours of the line, and the rest pale with no glow and no core, under the part', () => {
        // The part is the segment from x = 100 to x = 200.
        const strokes = strokesOf([100, 200]), inPart = strokes.filter(stroke => stroke.from === 100), rest = strokes.filter(stroke => stroke.from !== 100);
        expect(rest.length).toBeGreaterThan(0);
        expect(rest.every(stroke => stroke.colour === REST.route && stroke.alpha === 1)).toBe(true);
        expect(new Set(inPart.map(stroke => stroke.colour))).toEqual(new Set([GLOW, LINE, CORE]));
        expect(inPart.find(stroke => stroke.colour === LINE)?.alpha).toBe(1);
        expect(strokes.indexOf(inPart[0])).toBeGreaterThan(strokes.lastIndexOf(rest.at(-1)!));
    });

    it('draws a part shorter than one segment in the colour of the line', () => {
        const strong = strokesOf([120, 130]).filter(stroke => stroke.colour === LINE);
        expect(strong.map(stroke => stroke.from)).toEqual([100]);
    });
});
