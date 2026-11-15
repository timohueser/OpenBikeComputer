import { describe, expect, it } from 'vitest';
import type { Summary } from '../library/records';
import { creditParts, drawCard, layout, SIZES, stretch, type Box, type Card } from './card';

const summary = {
    line: [8, 46, 8.1, 46, 8.2, 46], heights: [400, 900, 500], bounds: [8, 46, 8.2, 46],
    lengthM: 15_400, climbM: 500, highestM: 900, seconds: 3600, bike: 'road',
} satisfies Summary;
const card: Card = { name: 'Lake test', look: 'route', summary, numbers: [['15.4', 'km'], ['500', 'm up'], ['1h 0m', '']], line: true, profile: true, trim: 0 };

/** A context that keeps each text. A letter is half as wide as its size. */
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
    return { context: context as unknown as CanvasRenderingContext2D, texts };
}
const bottom = (box: Box) => box.y + box.height;

describe('the layout of the wide share card', () => {
    const [width, height] = SIZES.wide;
    const all = layout('wide', { nameLines: 2, date: true, numbers: true, profile: true, creditLines: 2, creditUnder: true });

    it('fills the card with the picture, puts the band on the lower edge and the text over it, inside the margins', () => {
        expect([width, height]).toEqual([1920, 1280]);
        expect(all.picture).toEqual({ x: 0, y: 0, width, height });
        expect(all.band).toMatchObject({ x: 0, width });
        expect(bottom(all.band!)).toBe(height);
        const rows = [all.mark, all.date!, all.name, all.numbers!, all.credit!, all.band!];
        // Each row ends before the next one starts, from the mark down to the band.
        rows.slice(1).forEach((row, i) => expect(row.y).toBeGreaterThanOrEqual(bottom(rows[i])));
        for (const row of rows.slice(0, 5)) {
            expect(row.x).toBe(all.mark.x);
            expect(row.x + row.width).toBeLessThanOrEqual(width - all.mark.x);
        }
        // The line on a plain ground has its room between the mark and the text.
        expect(all.plan.y).toBeGreaterThanOrEqual(bottom(all.mark));
        expect(bottom(all.plan)).toBeLessThan(all.date!.y);
    });

    it('gives a part that is off no box, and its place to the part over it', () => {
        const plain = layout('wide', { nameLines: 1, date: false, numbers: false, profile: false, creditLines: 0, creditUnder: false });
        expect([plain.date, plain.numbers, plain.credit, plain.band]).toEqual([undefined, undefined, undefined, undefined]);
        // With no band, the name keeps the margin of the sides from the lower edge.
        expect(height - bottom(plain.name)).toBe(plain.name.x);
        const numbered = layout('wide', { nameLines: 1, date: false, numbers: true, profile: false, creditLines: 0, creditUnder: false });
        expect(bottom(numbered.numbers!)).toBe(bottom(plain.name));
        expect(bottom(numbered.name)).toBeLessThan(numbered.numbers!.y);
    });
});

describe('the drawing of a share card', () => {
    it('draws the name, each number with its unit and the mark; with no numbers, it draws no number', () => {
        const full = recorder(), bare = recorder();
        drawCard(full.context, 'wide', card);
        drawCard(bare.context, 'wide', { ...card, numbers: [] });
        expect(full.texts).toEqual(['Lake test', '15.4', ' km', '500', ' m up', '1h 0m', 'OpenBikeComputer']);
        expect(bare.texts).toEqual(['Lake test', 'OpenBikeComputer']);
    });

    it('breaks a long name between words into at most three lines, and ends the rest with "…"', () => {
        const { context, texts } = recorder();
        // A letter of the name is 48 px wide, so a line holds 36 letters.
        drawCard(context, 'wide', { ...card, name: Array.from({ length: 30 }, () => 'mountain').join(' '), numbers: [] });
        expect(texts.slice(0, 3)).toEqual(['mountain mountain mountain mountain', 'mountain mountain mountain mountain', 'mountain mountain mountain mountain…']);
        expect(texts).toHaveLength(4);
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

describe('the credits of a share card', () => {
    it('keeps each part between two ";" whole, also a part with no "©", and removes a second copy', () => {
        expect(creditParts([
            '© swisstopo (Open data, attribution required); produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018; © OpenStreetMap contributors',
            '© Environment Agency copyright and/or database right 2022. All rights reserved.',
            '© GeoBasis-DE/M-V 2024; © GeoBasis-DE / LVermGeo LSA',
            '© European Union, Copernicus Land Monitoring Service 2026, European Environment Agency (EEA): HR-WSI Snow Phenology',
            'Contains modified Copernicus Sentinel data 2025; © OpenStreetMap contributors; © 2024 X',
        ])).toEqual([
            '© swisstopo (Open data, attribution required)',
            'produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018',
            '© OpenStreetMap contributors',
            '© Environment Agency copyright and/or database right 2022. All rights reserved.',
            '© GeoBasis-DE/M-V 2024',
            '© GeoBasis-DE / LVermGeo LSA',
            '© European Union, Copernicus Land Monitoring Service 2026, European Environment Agency (EEA): HR-WSI Snow Phenology',
            'Contains modified Copernicus Sentinel data 2025',
            '© 2024 X',
        ]);
    });

    it('puts short credits beside the text, and long credits whole under it in smaller letters', () => {
        const cover = { image: {} as CanvasImageSource, width: 1920, height: 1280, points: { x: new Float32Array(3), y: new Float32Array(3), eye: new Uint8Array(3) } };
        const short = recorder(), long = recorder();
        const beside = drawCard(short.context, 'wide', { ...card, line: false, cover: { ...cover, credits: ['© swisstopo; © OpenStreetMap contributors'] } });
        expect(short.texts).toContain('© swisstopo\u00a0· © OpenStreetMap contributors');
        expect(beside.credit!.y + beside.credit!.height).toBe(beside.numbers!.y + beside.numbers!.height);
        const sentence = 'produced using Copernicus WorldDEM-30 and provided under COPERNICUS by the European Union and ESA';
        const under = drawCard(long.context, 'wide', { ...card, line: false, cover: { ...cover, credits: Array.from({ length: 12 }, (_, i) => `${sentence} ${i}`) } });
        const lines = long.texts.filter(text => text.startsWith('produced') || text.includes('COPERNICUS'));
        // No line ends with "…", and the last one holds the end of the last credit.
        expect(lines.join(' ')).not.toContain('…');
        expect(long.texts.some(text => text.endsWith('ESA 11'))).toBe(true);
        expect(under.letters.small).toBeLessThan(beside.letters.small);
        expect(under.credit!.y).toBeGreaterThan(under.numbers!.y + under.numbers!.height);
    });
});
