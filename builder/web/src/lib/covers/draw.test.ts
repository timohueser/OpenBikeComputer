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

/** A context that keeps the colour of each full-strength stroke. */
function recorder() {
    const strokes: string[] = [];
    const context = {
        globalAlpha: 1, strokeStyle: '', fillStyle: '', lineWidth: 1, lineCap: '', lineJoin: '',
        drawImage() {}, beginPath() {}, moveTo() {}, lineTo() {}, arc() {}, fill() {},
        stroke() { if (context.globalAlpha === 1) strokes.push(context.strokeStyle); },
    };
    return { context: context as unknown as CanvasRenderingContext2D, strokes };
}

describe('the line of a cover', () => {
    it('draws the part in the colour of the line and the rest paler', () => {
        const cover = { image: {} as CanvasImageSource, width: 400, height: 100, points, summary };
        const whole = recorder(), withPart = recorder();
        drawCover(whole.context, cover, { box: [400, 100], line: true });
        drawCover(withPart.context, cover, { box: [400, 100], line: true, part: [100, 200] });
        expect(whole.strokes).not.toContain(REST.route);
        expect(withPart.strokes).toContain(REST.route);
        // The part keeps the colour of the line.
        expect(withPart.strokes).toContain('#cc2a93');
    });
});
