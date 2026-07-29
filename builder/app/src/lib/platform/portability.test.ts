// The domain-move guard for C6 (#905): the static web tier is published today under
// GitHub Pages' project sub-path (`/OpenBikeComputer/builder/`) and is expected to
// move to its own domain. Nothing in the built bundle may assume either one.
//
// Like bundle.test.ts, this asserts against what Rollup actually emitted rather than
// against source — the claim is about the shipped files, so the shipped files are what
// gets read. Two properties, and they fail differently:
//
//   * asset URLs are relative — a bundle with `/assets/…` in its HTML is
//     mounted-at-root-only and would 404 under any prefix. Vite's `base: "./"` (#895)
//     is what makes this true; this test is what keeps it true.
//   * built the way the deploy builds it, no absolute site URL survives anywhere. The
//     deploy passes VITE_SITE_BASE=../ because the docs and the landing page are
//     siblings in the same artifact; if a hard-coded github.io URL crept back into a
//     component, every link would keep working right up until the move.

import { build } from "vite";
import { describe, expect, it } from "vitest";

/** The slice of Rollup's single-input result we need: emitted chunk code and asset
 *  sources, keyed by filename. */
interface BuiltOutput {
    output: Array<{ type: string; fileName: string; code?: string; source?: string | Uint8Array }>;
}

/** Build the web target with `env` applied, and return the text of every emitted
 *  chunk and text asset. Vite reads VITE_-prefixed variables off `process.env`, so
 *  this is the same input the workflow gives it. */
async function buildWeb(env: Record<string, string> = {}): Promise<Map<string, string>> {
    const saved = new Map(Object.keys(env).map((k) => [k, process.env[k]]));
    Object.assign(process.env, env);
    try {
        const out = await build({
            mode: "web",
            logLevel: "error",
            build: { write: false, outDir: "dist/.portability-test" },
        });
        const result = (Array.isArray(out) ? out[0] : out) as unknown as BuiltOutput;
        const files = new Map<string, string>();
        for (const f of result.output) {
            const text = f.type === "chunk" ? f.code : typeof f.source === "string" ? f.source : "";
            if (text) files.set(f.fileName, text);
        }
        return files;
    } finally {
        for (const [k, v] of saved) {
            if (v === undefined) delete process.env[k];
            else process.env[k] = v;
        }
    }
}

/** Filenames whose text matches `needle`. */
function offenders(files: Map<string, string>, needle: string | RegExp): string[] {
    const hit =
        typeof needle === "string"
            ? (t: string) => t.includes(needle)
            : (t: string) => needle.test(t);
    return [...files].filter(([, text]) => hit(text)).map(([name]) => name);
}

/**
 * A *path-absolute* reference to the project-Pages sub-path — `"/OpenBikeComputer/…"`
 * — which is the thing that breaks on a move. Deliberately not a bare substring
 * search: `https://github.com/timohueser/OpenBikeComputer/releases` contains the same
 * characters and is a perfectly good link to a repository. The preceding quote or
 * bracket is what distinguishes "a URL path starting at the origin root" from "part of
 * some other URL". Same rule as the deploy's own grep.
 */
const ROOTED_SITE_PATH = /["'`(=,]\/OpenBikeComputer\//;

describe("deployment portability", () => {
    it("references its own assets relatively", async () => {
        const html = (await buildWeb()).get("index.html") ?? "";
        // The build produced an index.html with script/style refs at all…
        expect(html).toMatch(/<script[^>]+src="/);
        // …and every local ref in it is relative, not rooted at "/".
        const refs = [...html.matchAll(/(?:src|href)="([^"]+)"/g)].map((m) => m[1]);
        const local = refs.filter((r) => !/^[a-z]+:/.test(r) && !r.startsWith("//"));
        expect(local.length).toBeGreaterThan(0);
        expect(local.filter((r) => r.startsWith("/"))).toEqual([]);
    }, 180_000);

    it("bakes in no absolute site URL when built the way the deploy builds it", async () => {
        // Exactly what .github/workflows/deploy-site.yml passes.
        const files = await buildWeb({ VITE_SITE_BASE: "../" });
        expect(offenders(files, "timohueser.github.io")).toEqual([]);
        expect(offenders(files, "openbikecomputer.com")).toEqual([]);
        expect(offenders(files, ROOTED_SITE_PATH)).toEqual([]);
    }, 180_000);
});
