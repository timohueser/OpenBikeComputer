import { createHash } from 'node:crypto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CatalogRecord } from '../planner/signed-routes';

const LOOK = '1|0715-1000|960|640', SET = '0123456789abcdef', CELL = '9-267-177';
const HOST = 'https://maps.openbikecomputer.com/route-pictures';
const record = (id: number, line: number[]) => ({ id, cells: [CELL], line_udeg: line }) as CatalogRecord;
const keyOf = (line: number[]) => createHash('sha256').update(`${LOOK}|${line.join(',')}`).digest('hex').slice(0, 16);
const LINE = [7_850_000, 47_990_000, 1000, 500];
const KEY = keyOf(LINE);
const manifest = { format: 1, set: SET, look: LOOK, months: { '07': '0715-1000' }, default_month: '07', credits: ['© Data'] };

/** The files of the host by path; each request is counted. */
let files: Record<string, unknown>, asked: string[];
beforeEach(() => {
    vi.resetModules();
    asked = [];
    files = {
        '/manifest.json': manifest,
        [`/sets/${SET}/cells/${CELL}.json`]: { format: 1, routes: { 1: KEY, 2: 'ffffffffffffffff' } },
        [`/covers/1/${KEY}-0715-1000.webp`]: 'image',
        [`/covers/1/${KEY}.json`]: { x: [10, null], y: [20, null], eye: [130, 0] },
    };
    vi.stubGlobal('fetch', async (url: string) => {
        const path = url.slice(HOST.length);
        asked.push(path);
        const body = files[path];
        return body === undefined ? new Response(null, { status: 404 }) : new Response(typeof body === 'string' ? body : JSON.stringify(body));
    });
    vi.stubGlobal('createImageBitmap', async () => ({ close() {} }));
});
afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
});

describe('coverOf', () => {
    it('reads one cell file for two rows that ask at the same time, and makes a null place a point with no place', async () => {
        const { coverOf } = await import('./route-pictures');
        const [first, second] = await Promise.all([coverOf(record(1, LINE)), coverOf(record(2, LINE))]);
        expect(asked.filter(path => path.includes('/cells/'))).toHaveLength(1);
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
});
