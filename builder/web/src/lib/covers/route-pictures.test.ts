import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CatalogRecord } from '../planner/signed-routes';

const LOOK = '1|0715-1000|960|640', SET = '0123456789abcdef', CELL = '9-267-177';
const HOST = 'https://maps.openbikecomputer.com/route-pictures';
const record = (id: number, line: number[]) => ({ id, cells: [CELL], line_udeg: line }) as CatalogRecord;
const LINE = [7_850_000, 47_990_000, 1000, 500];
/** `keyOf` of the cover script (tests/browser/trailview/route-pictures.mjs) for LOOK and LINE. */
const KEY = '0689c7e60606ce1b';
const manifest = { format: 1, set: SET, look: LOOK, months: { '07': '0715-1000' }, default_month: '07', credits: ['© Data'] };
const CELL_FILE = `/sets/${SET}/cells/${CELL}.json`;

/** The files of the host by path: a body, a status, or an error to throw. Each request is counted. */
let files: Record<string, unknown>, asked: string[];
beforeEach(() => {
    vi.resetModules();
    asked = [];
    files = {
        '/manifest.json': manifest,
        [CELL_FILE]: { format: 1, routes: { 1: KEY, 2: 'ffffffffffffffff' } },
        [`/covers/1/${KEY}-0715-1000.webp`]: 'image',
        [`/covers/1/${KEY}.json`]: { x: [10, null], y: [20, null], eye: [130, 0] },
    };
    vi.stubGlobal('fetch', async (url: string, init?: RequestInit) => {
        const path = url.slice(HOST.length);
        asked.push(path);
        if (init?.signal?.aborted) throw new DOMException('aborted', 'AbortError');
        const body = files[path];
        if (body instanceof Error) throw body;
        if (typeof body === 'number') return new Response(null, { status: body });
        return body === undefined ? new Response(null, { status: 404 }) : new Response(typeof body === 'string' ? body : JSON.stringify(body));
    });
    vi.stubGlobal('createImageBitmap', async () => ({ close() {} }));
});
afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
});
const cellReads = () => asked.filter(path => path === CELL_FILE).length;

describe('coverOf', () => {
    it('finds the cover by the key that the cover script writes, reads one cell file for two rows, and makes a null place a point with no place', async () => {
        const { coverOf } = await import('./route-pictures');
        const [first, second] = await Promise.all([coverOf(record(1, LINE)), coverOf(record(2, LINE))]);
        expect(cellReads()).toBe(1);
        expect(first?.credits).toEqual(['© Data']);
        expect(Array.from(first!.points.x)).toEqual([10, NaN]);
        expect(Array.from(first!.points.eye)).toEqual([130, 0]);
        // The cell file holds another key for this line.
        expect(second).toBeUndefined();
    });

    it('gives nothing for a record whose line changed, and loads no file for it', async () => {
        const { coverOf } = await import('./route-pictures');
        expect(await coverOf(record(1, [...LINE, 7, 7]))).toBeUndefined();
        expect(asked.some(path => path.startsWith('/covers/'))).toBe(false);
    });

    it('gives nothing when an image fails, and reads the manifest again after a 404, at most once in a minute', async () => {
        vi.useFakeTimers({ toFake: ['Date'] });
        const { coverOf, routePictures } = await import('./route-pictures');
        await routePictures();
        delete files[`/covers/1/${KEY}-0715-1000.webp`];
        vi.setSystemTime(Date.now() + 60_000);
        expect(await coverOf(record(1, LINE))).toBeUndefined();
        expect(await coverOf(record(1, LINE))).toBeUndefined();
        expect(asked.filter(path => path === '/manifest.json')).toHaveLength(2);
    });

    it('reads the manifest again a minute after a read with no signal, not before', async () => {
        vi.useFakeTimers({ toFake: ['Date'] });
        files['/manifest.json'] = new TypeError('Failed to fetch');
        const { coverOf } = await import('./route-pictures');
        expect(await coverOf(record(1, LINE))).toBeUndefined();
        files['/manifest.json'] = manifest;
        expect(await coverOf(record(1, LINE))).toBeUndefined();
        vi.setSystemTime(Date.now() + 60_000);
        expect(await coverOf(record(1, LINE))).toBeDefined();
        expect(asked.filter(path => path === '/manifest.json')).toHaveLength(2);
    });

    it('asks for a cell file again after a 503, and keeps one that answers 404', async () => {
        files[CELL_FILE] = 503;
        const { coverOf } = await import('./route-pictures');
        expect(await coverOf(record(1, LINE))).toBeUndefined();
        files[CELL_FILE] = { format: 1, routes: { 1: KEY } };
        expect(await coverOf(record(1, LINE))).toBeDefined();
        expect(cellReads()).toBe(2);
        vi.resetModules();
        delete files[CELL_FILE];
        const again = await import('./route-pictures');
        await again.coverOf(record(1, LINE));
        await again.coverOf(record(1, LINE));
        expect(cellReads()).toBe(3);
    });
});

describe('holdCover', () => {
    it('loads once for two holders, and the last release stops a load that has not ended', async () => {
        const { holdCover } = await import('./route-pictures');
        const route = record(1, LINE);
        const [a, b] = [holdCover(route), holdCover(route)];
        expect(a.cover).toBe(b.cover);
        a.release();
        b.release();
        expect(await b.cover).toBeUndefined();
        expect(asked.some(path => path.startsWith('/covers/'))).toBe(false);
    });
});
