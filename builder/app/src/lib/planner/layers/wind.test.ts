import { describe, expect, it } from 'vitest';
import { specTile } from '../../../../test-support/planner/climate-tiles';
import { DETAIL, OVERVIEW, SECTORS } from './climate';
import { lineBearings } from './climate-route';
import { HEADWIND_EDGES, NO_WIND, arrowAxis, arrowStride, headClass, headClasses, viewTiles, windChart, windMap, windYear } from './wind';

// Overview tile 78/26 holds Freiburg at cell 4 × 24 + 7, column 1879 and row 420.
const FREIBURG = 4 * 24 + 7, EAST = FREIBURG + 1, SOUTH = FREIBURG + 24;
const APRIL = 3, WEEK = 15;

/** Rose codes by the sector the wind comes from, in 0.5 % steps; the rest of 200 spreads evenly. */
function setRose(t: ReturnType<typeof specTile>, cell: number, from: Record<number, number>) {
    const rest = (200 - Object.values(from).reduce((a, b) => a + b, 0)) / (SECTORS - Object.keys(from).length);
    for (let month = 0; month < 12; month++) for (let s = 0; s < SECTORS; s++) t.set('rose', SECTORS * month + s, cell, from[s] ?? rest);
}

function overview() {
    const t = specTile(OVERVIEW, 78, 26);
    for (const [cell, code] of [[FREIBURG, 5], [EAST, 4], [SOUTH, 5]]) for (let week = 0; week < 52; week++) t.set('wind', week, cell, code); // 2.5 and 2 m/s
    setRose(t, FREIBURG, { 8: 170 }); // from the south: blows towards the north, steady
    setRose(t, EAST, { 4: 70, 12: 60 }); // from the east and the west: two opposite winds
    t.set('wind', WEEK, SOUTH, 255);
    return t.tile();
}

describe('wind map', () => {
    it('colours the cells from 2.5 m/s and points an arrow where the wind blows towards', () => {
        const { cells, arrows } = windMap([overview()], WEEK, APRIL, 1);
        expect(cells.features.map(f => f.properties.speed)).toEqual([2.5]);
        // Neighbour cells share their edge exactly, so the fill has no seams.
        const [north, south] = windMap([overview()], 0, APRIL, 1).cells.features.map(f => f.geometry.coordinates[0]);
        expect(north[0][1]).toBe(south[2][1]);
        expect(north[0]).toEqual([-180 + 0.1 * 1878.5, 90 - 0.1 * 420.5]);
        expect(arrows.features.map(f => f.properties)).toEqual([{ icon: 'wind-arrow-2', rotate: 0 }, { icon: 'wind-double-1', rotate: 270 }]);
        const [lon, lat] = arrows.features[0].geometry.coordinates;
        expect([lon, lat].map(v => v.toFixed(6))).toEqual(['7.900000', '48.000000']);
    });

    it('turns a double arrow to the mean axis of its two directions', () => {
        expect(arrowAxis({ towards: 4 })).toBe(90);
        expect(arrowAxis({ towards: 0, opposite: 8 })).toBe(0);
        // Towards north and south-south-west: the axis leans 11.25° east of north.
        expect(arrowAxis({ towards: 0, opposite: 9 })).toBe(11.25);
        expect(arrowAxis({ towards: 0, opposite: 7 })).toBe(348.75);
        expect(arrowAxis({ towards: 15, opposite: 5 })).toBe(315);
    });

    it('keeps the arrows of every second cell when the cells are narrow on screen', () => {
        expect([arrowStride(9), arrowStride(8.3), arrowStride(8), arrowStride(6)]).toEqual([1, 1, 2, 8]);
        // Column 1879 is odd: the stride drops Freiburg and keeps its east neighbour.
        expect(windMap([overview()], WEEK, APRIL, 2).arrows.features.map(f => f.properties.icon)).toEqual(['wind-double-1']);
    });

    it('requests the overview tiles of the view inside the archive', () => {
        const bounds = [7.45, 47.5, 10.5, 49.85];
        expect(viewTiles([7.7, 47.9, 8.1, 48.1], bounds)).toEqual([[78, 26]]);
        expect(viewTiles([6, 47, 9.6, 48.5], bounds)).toEqual([[78, 25], [79, 25], [78, 26], [79, 26]]);
        expect(viewTiles([11, 47, 12, 48], bounds)).toEqual([]);
    });
});

describe('wind along a route', () => {
    const cells = () => {
        const tile = overview();
        return [{ tile, index: FREIBURG }, { tile, index: FREIBURG }, undefined];
    };

    it('shades each sample by the chance of headwind for its travel direction', () => {
        // Towards the north at 85 % plus an even rest: riding south meets it head on.
        expect([...headClasses(cells(), [180, 0, 0], APRIL)]).toEqual([3, 0, NO_WIND]);
        expect([0.1, 0.15, 0.3, 0.45, NaN].map(headClass)).toEqual([0, 1, 2, HEADWIND_EDGES.length, NO_WIND]);
    });

    it('takes the travel direction across a stretch, so a hairpin keeps the main direction', () => {
        // Northbound with a 50 m kink to the east and back.
        const line: [number, number][] = [[8, 48], [8, 48.002], [8.0007, 48.002], [8, 48.002], [8, 48.004]];
        const km = [0, 0.22, 0.27, 0.32, 0.54];
        expect(Math.round(lineBearings(line)[3])).toBe(347);
        expect([...lineBearings(line, km, 0.2)].map(Math.round)).toEqual([0, 0, 0, 0, 0]);
    });

    it('shows the headwind chance of each month on the year slider', () => {
        const row = Float32Array.from({ length: 12 }, (_, month) => month / 12);
        const year = windYear(row, '2026-04-16', 'light');
        expect(year.label).toBe('Chance of headwind on the route in April: 25 %');
        expect(year.rows[0].cells[0]).toBe(0);
        expect(year.rows[0].cells[WEEK]).toBe(1);
        expect(year.rows[0].cells[51]).toBe(3);
        const none = windYear(null, '2026-04-16', 'light');
        expect(none.label).toBe('Plan a route to see the chance of headwind along it.');
        expect(none.rows[0].cells.every(cell => cell === 255)).toBe(true);
    });
});

describe('wind at a point', () => {
    const detail = () => {
        const t = specTile(DETAIL, 156, 52);
        for (let i = 0; i < 520; i++) t.set('wind', i, 4 * 12 + 7, i % 52 === WEEK ? 9 : 4);
        t.set('wind', 52 * 9 + WEEK, 4 * 12 + 7, 255);
        return { tile: t.tile(), index: 4 * 12 + 7 };
    };

    it('gives each week of ten years as its daytime mean speed, and the rose of the month', () => {
        const { chart, rose } = windChart({ overview: { tile: overview(), index: FREIBURG }, detail: detail() }, 2016, '2026-04-16', 'light');
        expect(chart.headline).toBe('In April the wind most often blows towards N');
        const grid = chart.grids[0];
        expect(grid.label).toBe('Daytime wind around 16 Apr: 2.5 m/s on average');
        expect(grid.rows.map(row => row.label)).toEqual(Array.from({ length: 10 }, (_, year) => String(2016 + year)));
        // Codes count 0.5 m/s: 4.5 m/s is code 9, 2 m/s is code 4, a missing week is empty.
        expect([grid.rows[0].cells[WEEK], grid.rows[0].cells[0], grid.rows[9].cells[WEEK]]).toEqual([9, 4, 255]);
        expect(rose?.month).toBe('April');
        expect(rose?.shares[0]).toBeCloseTo(0.85);
    });

    it('gives the headwind and tailwind chances for a travel direction', () => {
        const { chart, rose } = windChart({ overview: { tile: overview(), index: FREIBURG }, bearing: 180 }, 2016, '2026-04-16', 'light');
        expect(chart.headline).toBe('Headwind in 88 % of April daytime hours, tailwind in 4 %');
        expect(chart.grids).toEqual([]);
        expect(rose?.bearing).toBe(180);
        expect(windChart({}, 2016, '2026-04-16', 'light')).toEqual({ chart: { headline: 'No wind data here', grids: [], note: '~9 km grid, daytime wind (09–18 h)' } });
    });
});
