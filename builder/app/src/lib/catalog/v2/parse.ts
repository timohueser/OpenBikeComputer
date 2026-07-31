// The primitive validators the v2 document parsers are built from.
//
// These are deliberately the same shapes the v1 parser (`../manifest.ts`) grew
// privately, and they are deliberately *not* shared with it. v1 is frozen: the
// live site runs on it until the cutover, after which it is deleted whole. A
// shared helper module would therefore be a module with one consumer a release
// from now, and in the meantime it would couple two parsers that must be free to
// disagree — v2 rejects things v1 accepts (a `presets` key, for one) precisely
// because the envelope moved on.
//
// What *is* shared is the error type: a caller catching a bad catalog should not
// have to know which envelope it came from.

import { CatalogFormatError } from "../manifest";

export { CatalogFormatError };

export type Obj = Record<string, unknown>;

/** Kebab id, as `OBCC_Spec.md` §11 spells every id it constrains. */
export const KEBAB = /^[a-z0-9]+(-[a-z0-9]+)*$/;
/** Slash-separated region / extract id. */
export const PATH_ID = /^[a-z0-9]+(-[a-z0-9]+)*(\/[a-z0-9]+(-[a-z0-9]+)*)*$/;
export const SHA256 = /^[0-9a-f]{64}$/;
/** §5: exactly one spelling — twenty characters, `Z`, no fractional seconds. */
export const INSTANT = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/;
export const DATE = /^\d{4}-\d{2}-\d{2}$/;

export function fail(what: string): never {
    throw new CatalogFormatError(what);
}

export function obj(v: unknown, where: string): Obj {
    if (typeof v !== "object" || v === null || Array.isArray(v)) fail(`${where}: expected an object`);
    return v as Obj;
}

export function arr(v: unknown, where: string): unknown[] {
    if (!Array.isArray(v)) fail(`${where}: expected an array`);
    return v;
}

export function str(o: Obj, key: string, where: string, pattern?: RegExp): string {
    const v = o[key];
    if (typeof v !== "string" || v.length === 0) fail(`${where}: ${key} must be a non-empty string`);
    if (pattern && !pattern.test(v)) fail(`${where}: ${key} is malformed (${JSON.stringify(v)})`);
    return v;
}

export function int(o: Obj, key: string, where: string, min = 0, max = Number.MAX_SAFE_INTEGER): number {
    const v = o[key];
    if (typeof v !== "number" || !Number.isInteger(v) || v < min || v > max) {
        fail(`${where}: ${key} must be an integer in ${min}..=${max}`);
    }
    return v as number;
}

export function bool(o: Obj, key: string, where: string): boolean {
    const v = o[key];
    if (typeof v !== "boolean") fail(`${where}: ${key} must be a boolean`);
    return v;
}

/**
 * A URL field, under §3's rule: absolute `https://…`/`http://…`, or
 * root-relative `/…`.
 *
 * One implementation for every document that carries one — the root's satellite
 * refs, a region's `cells_url`, and every cell artifact in a band index — because
 * §11.6 says a cell's `url` is "resolved like v1's `url` (§3)" and a second
 * spelling of that rule is a second chance to accept a relative path. Resolution
 * itself is the client's, never a parser's: a parser reaching for a base URL
 * would be making up a fact the document does not contain.
 */
export function urlStr(o: Obj, key: string, where: string): string {
    const v = str(o, key, where);
    if (!/^(https?:\/\/|\/)/.test(v)) {
        fail(`${where}: ${key} must be absolute or root-relative (${JSON.stringify(v)})`);
    }
    return v;
}

/** An optional string that may also be spelled as an explicit `null`; both mean
 *  absent, which is how the generator writes "no preview yet" and "no parent". */
export function optionalStr(o: Obj, key: string, where: string, pattern?: RegExp): string | null {
    if (o[key] === undefined || o[key] === null) return null;
    return str(o, key, where, pattern);
}

/** A calendar date that exists — §5 rejects `2026-02-30` and `2023-02-29`. */
export function realDate(spelling: string, where: string): void {
    const [y, m, d] = spelling.slice(0, 10).split("-").map(Number);
    const probe = new Date(Date.UTC(y, m - 1, d));
    if (probe.getUTCFullYear() !== y || probe.getUTCMonth() !== m - 1 || probe.getUTCDate() !== d) {
        fail(`${where}: ${spelling} is not a real date`);
    }
}

/** A §5 timestamp: the one spelling, and a date that exists. */
export function instant(o: Obj, key: string, where: string): string {
    const v = str(o, key, where, INSTANT);
    realDate(v, `${where}.${key}`);
    return v;
}

/** Whole body in, parsed tree out. The seam OBCC §7 draws: a document is read
 *  entire and parsed as one JSON value, because JSON is self-delimiting and no
 *  proper prefix of a valid document parses. Nothing incremental, ever. */
export function json(body: string, where: string): unknown {
    try {
        return JSON.parse(body);
    } catch (e) {
        fail(`${where}: not a JSON document: ${e instanceof Error ? e.message : String(e)}`);
    }
}

/** A `{ "<kebab id>": integer }` map, as `bytes_by_band` and `cell_count` are. */
export function intMap(v: unknown, where: string): Record<string, number> {
    const o = obj(v, where);
    const out: Record<string, number> = {};
    for (const key of Object.keys(o)) {
        if (!KEBAB.test(key)) fail(`${where}: ${JSON.stringify(key)} is not a band id`);
        out[key] = int(o, key, where, 0);
    }
    return out;
}
