import { describe, expect, it } from 'vitest';
import type { Summary } from '../library/records';
import { drawCard, layout, stretch, type Box, type Card } from './card';

const summary = {
    line: [8, 46, 8.1, 46, 8.2, 46], heights: [400, 900, 500], bounds: [8, 46, 8.2, 46],
    lengthM: 15_400, climbM: 500, highestM: 900, seconds: 3600, bike: 'road',
} satisfies Summary;
const card: Card = { name: 'Lake test', look: 'route', summary, numbers: [['15.4', 'km'], ['500', 'm up'], ['1h 0m', '']], line: true, profile: true, trim: 0 };
const cover = { image: {} as CanvasImageSource, width: 1920, height: 1280, points: { x: new Float32Array(3), y: new Float32Array(3), eye: new Uint8Array(3) } };

/** A canvas that keeps each text. A letter is half as wide as its size. */
function recorder() {
    const texts: string[] = [];
    const size = () => Number(/(\d+)px/.exec(target.font)![1]);
    const target = {
        font: '10px',
        measureText: (text: string) => ({ width: text.length * size() / 2 }),
        fillText: (text: string) => { texts.push(text); },
        createLinearGradient: () => ({ addColorStop() {} }),
    };
    const context = new Proxy(target as Record<string, unknown>, { get: (kept, key: string) => key in kept ? kept[key] : () => {} });
    const canvas = { width: 0, height: 0, getContext: () => context };
    return { canvas: canvas as unknown as HTMLCanvasElement, texts };
}
const bottom = (box: Box) => box.y + box.height;

describe('the layout of the wide share card', () => {
    const all = layout('wide', { nameLines: 3, date: true, numbers: true, profile: true, credit: true });

    // The usual card of a ride, and the highest card.
    it.each([1, 3])('has the whole picture at the top with only the mark on it, and each row and the profile under it: %i name lines', nameLines => {
        const boxes = nameLines === 3 ? all : layout('wide', { nameLines, date: true, numbers: true, profile: true, credit: true });
        expect(boxes.picture).toEqual({ x: 0, y: 0, width: 1920, height: 1280 });
        expect(boxes.ground).toMatchObject({ x: 0, y: 1280, width: 1920 });
        expect(boxes.size).toEqual([1920, bottom(boxes.ground)]);
        expect(bottom(boxes.mark)).toBeLessThan(boxes.plan.y);
        expect(bottom(boxes.plan)).toBeLessThan(boxes.ground.y);
        // Each row ends before the next one starts, from the top of the ground down to the profile on its lower edge.
        const rows = [boxes.picture, boxes.date!, boxes.name, boxes.numbers!, boxes.credit!, boxes.band!];
        rows.slice(1).forEach((row, i) => expect(row.y).toBeGreaterThanOrEqual(bottom(rows[i])));
        for (const row of rows.slice(1, 5)) expect([row.x, row.x + row.width]).toEqual([boxes.mark.x, 1920 - boxes.mark.x]);
        expect(boxes.band).toMatchObject({ x: 0, width: 1920 });
        expect(bottom(boxes.band!)).toBe(boxes.size[1]);
        // With each row, the text leaves a profile of 110 pixels.
        expect(boxes.band!.height).toBe(110);
    });

    it('has one size for each count of name lines, and gives the height that a row leaves to the profile', () => {
        const sizes = [true, false].map(on => layout('wide', { nameLines: 1, date: on, numbers: on, profile: on, credit: on }).size);
        expect(sizes).toEqual([[1920, 1780], [1920, 1780]]);
        expect(all.size).toEqual([1920, 1988]);
        const bare = layout('wide', { nameLines: 1, date: false, numbers: false, profile: true, credit: false });
        expect([bare.date, bare.numbers, bare.credit]).toEqual([undefined, undefined, undefined]);
        // The profile is at most 200 pixels high.
        expect(bare.band!.height).toBe(200);
    });

    it('puts the text in the middle of the ground with no profile', () => {
        const plain = layout('wide', { nameLines: 1, date: false, numbers: true, profile: false, credit: false });
        expect(plain.band).toBeUndefined();
        // From the top of the capital letters of the name to the baseline of the numbers.
        const top = plain.name.y + plain.name.height - 0.7 * plain.letters.name;
        expect(Math.abs(top - plain.ground.y - (bottom(plain.ground) - bottom(plain.numbers!)))).toBeLessThanOrEqual(1);
    });
});

describe('the layout of the square and the tall share card', () => {
    const rows = { date: true, numbers: true, profile: true, credit: true };

    it.each([['square', [1280, 1280], 853], ['tall', [1024, 1280], 683]] as const)('has one size and the whole picture at the top, and each row under it: %s', (format, size, pictureHeight) => {
        for (const nameLines of [1, 3]) {
            const boxes = layout(format, { nameLines, ...rows });
            expect(boxes.size).toEqual(size);
            // The picture is 3 : 2, as a cover, so the card cuts nothing of it.
            expect(boxes.picture).toEqual({ x: 0, y: 0, width: size[0], height: pictureHeight });
            expect(boxes.ground).toEqual({ x: 0, y: pictureHeight, width: size[0], height: size[1] - pictureHeight });
            const parts = [boxes.picture, boxes.date!, boxes.name, boxes.numbers!, boxes.credit!, ...boxes.band ? [boxes.band] : []];
            parts.slice(1).forEach((part, i) => expect(part.y).toBeGreaterThanOrEqual(bottom(parts[i])));
            // The text keeps the margin from the lower edge, and a profile ends on it.
            expect(size[1] - bottom(boxes.credit!)).toBeGreaterThanOrEqual(boxes.mark.x);
            if (boxes.band) expect(bottom(boxes.band)).toBe(size[1]);
        }
    });

    it('makes the profile lower when the text needs the room, and leaves it out when it gets too low', () => {
        const heights = (format: 'square' | 'tall') => [1, 2, 3].map(nameLines => layout(format, { nameLines, ...rows }).band?.height);
        expect(heights('square')).toEqual([169, 100, undefined]);
        expect(heights('tall')).toEqual([239, 239, 239]);
        // The layout says so also with the profile off, so that the window can leave out its switch.
        expect([1, 3].map(nameLines => layout('square', { nameLines, ...rows, profile: false }).profileFits)).toEqual([true, false]);
    });
});

describe('the drawing of a share card', () => {
    it('gives the canvas the size of the card, and draws the name, each number with its unit and the mark', () => {
        const full = recorder(), bare = recorder();
        drawCard(full.canvas, 'wide', card);
        drawCard(bare.canvas, 'wide', { ...card, numbers: [] });
        expect([full.canvas.width, full.canvas.height]).toEqual([1920, 1780]);
        expect(full.texts).toEqual(['Lake test', '15.4', ' km', '500', ' m up', '1h 0m', 'OpenBikeComputer']);
        // With no numbers, no number is drawn.
        expect(bare.texts).toEqual(['Lake test', 'OpenBikeComputer']);
    });

    it('has the one credit line only with a picture', () => {
        const { canvas, texts } = recorder();
        drawCard(canvas, 'wide', { ...card, line: false, cover });
        expect(texts).toEqual(['Lake test', '15.4', ' km', '500', ' m up', '1h 0m', '© OpenStreetMap contributors and others', 'OpenBikeComputer']);
    });

    it('breaks a long name between words into at most three lines, and ends the rest with "…"', () => {
        const { canvas, texts } = recorder();
        // A letter of the name is 48 px wide, so a line holds 36 letters.
        drawCard(canvas, 'wide', { ...card, name: Array.from({ length: 30 }, () => 'mountain').join(' '), numbers: [] });
        expect(texts.slice(0, 3)).toEqual(['mountain mountain mountain mountain', 'mountain mountain mountain mountain', 'mountain mountain mountain mountain…']);
        expect(texts).toHaveLength(4);
        expect(canvas.height).toBe(1988);
    });

    it('cuts the line of a plain card inside a segment, at the trimmed metres from each end', () => {
        // Two segments of 7.7 km along one latitude: a tenth of the line goes at each end.
        const cut = stretch(summary, 1540);
        expect(cut.map(([lon]) => Number(lon.toFixed(6)))).toEqual([8.02, 8.1, 8.18]);
        // A short stretch around the middle point.
        expect(stretch(summary, 7000).map(([lon]) => Number(lon.toFixed(4)))).toEqual([8.0909, 8.1, 8.1091]);
        expect(stretch(summary, 0)).toEqual([[8, 46], [8.1, 46], [8.2, 46]]);
    });
});
