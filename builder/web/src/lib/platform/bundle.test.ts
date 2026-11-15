// The build-product guard: every claim this repository makes about the files
// Vite actually emits is asserted here, against what Rollup put in the emitted
// output rather than against source imports or a grep of them.
//
// One file, because Vitest isolates by file and not by test. Two builds,
// because `web` and `production` (the dev host) are two different module graphs.

import { build } from "vite";
import { describe, expect, it } from "vitest";
import { registryAttribution } from "../../../vite/third-party-licenses";

/** The slice of Rollup's single-input result the four blocks read — per chunk
 *  `{ fileName, isEntry, modules, code }`, per asset `{ fileName, source }`.
 *  Vite re-exports the builder but not this type. */
type Built = Array<{
    type: string;
    fileName: string;
    isEntry?: boolean;
    modules?: Record<string, unknown>;
    code?: string;
    source?: string | Uint8Array;
}>;

const products = new Map<string, Promise<Built>>();
let queue: Promise<unknown> = Promise.resolve();

/**
 * One build per distinct product, memoised by mode and env, and serialised:
 * Vite reads VITE_-prefixed variables off the one `process.env` this process
 * has, so two builds awaited concurrently would read each other's.
 *
 * `write: false` keeps this off disk; outDir is redirected anyway so a future
 * Vite that prepares the directory before writing can't wipe a real build.
 */
function product(mode: string, env: Record<string, string> = {}): Promise<Built> {
    const key = `${mode} ${JSON.stringify(env)}`;
    const hit = products.get(key);
    if (hit) return hit;
    const next = queue.then(async (): Promise<Built> => {
        const saved = new Map(Object.keys(env).map((k) => [k, process.env[k]]));
        Object.assign(process.env, env);
        try {
            const out = await build({
                mode,
                logLevel: "error",
                build: { write: false, outDir: "dist/.bundle-test" },
            });
            return ((Array.isArray(out) ? out[0] : out) as unknown as { output: Built }).output;
        } finally {
            for (const [k, v] of saved) {
                if (v === undefined) delete process.env[k];
                else process.env[k] = v;
            }
        }
    });
    queue = next.then(
        () => undefined,
        () => undefined,
    );
    products.set(key, next);
    return next;
}

/** The static web tier as `.github/workflows/deploy-site.yml` builds it, and the
 *  maintainer dev host. `VITE_SITE_BASE` only sets the link
 *  prefix in `lib/constants.ts`, so the deployed build stands for `npm run build:web`
 *  too. */
const web = () => product("web", { VITE_SITE_BASE: "../" });
const dev = () => product("production");

/** Every source module in every emitted chunk of one product, and only the ones
 *  Rollup put in an entry chunk. */
const chunks = (b: Built) => b.filter((f) => f.type === "chunk");
const modulesOf = (b: Built) => chunks(b).flatMap((f) => Object.keys(f.modules ?? {}));
const entryModulesOf = (b: Built) =>
    chunks(b)
        .filter((f) => f.isEntry)
        .flatMap((f) => Object.keys(f.modules ?? {}));

/** The text of every emitted chunk and text asset, keyed by filename. */
function textFiles(b: Built): Map<string, string> {
    const files = new Map<string, string>();
    for (const f of b) {
        const text = f.type === "chunk" ? f.code : typeof f.source === "string" ? f.source : "";
        if (text) files.set(f.fileName, text);
    }
    return files;
}

/** Source modules the static web tier must not contain: only the dev host has them. */
const DEV_ONLY = {
    "the FastAPI client": /\/src\/lib\/api\/client\.ts$/,
    "the dev host": /\/src\/lib\/platform\/dev\.ts$/,
    "the style editor route": /\/src\/routes\/Advanced\.svelte/,
    "the style editor components": /\/src\/components\/advanced\//,
};

describe("bundle split", () => {
    it("keeps build and style-editor code out of the web target", async () => {
        const modules = modulesOf(await web());
        // An empty build, or one that quietly picked another host, would pass
        // every assertion below.
        expect(modules.some((id) => id.endsWith("/src/lib/platform/web.ts"))).toBe(true);
        expect(modules.some((id) => id.includes("/src/routes/Home.svelte"))).toBe(true);

        const found = Object.entries(DEV_ONLY).flatMap(([what, re]) =>
            modules.filter((id) => re.test(id)).map((id) => `${what}: ${id}`),
        );
        expect(found).toEqual([]);

        expect(modules.some((id) => id.includes("/src/components/coverage/CoverageHome.svelte"))).toBe(
            true,
        );
    }, 180_000);

    it("ships the maintainer style editor only in the dev target", async () => {
        const modules = modulesOf(await dev());
        const missing = Object.entries(DEV_ONLY)
            .filter(([, re]) => !modules.some((id) => re.test(id)))
            .map(([what]) => what);
        expect(missing).toEqual([]);

        // Localhost is a secure WebUSB context, so this build keeps the same lazy
        // browser transport.
        expect(modules.some((id) => id.endsWith("/src/lib/usb/webusb.ts"))).toBe(true);
    }, 180_000);
});

// Whatever the bundler puts in the bundle, its licence text has to ship beside
// it. A list of `dependencies` would prove nothing: Svelte is a devDependency
// compiled into every chunk.

const LICENSE_FILE = "third-party-licenses.txt";

/** The emitted notice file of one product, or "" if it emitted none. */
const noticesOf = (b: Built) =>
    String(b.find((f) => f.type === "asset" && f.fileName === LICENSE_FILE)?.source ?? "");

/** Every npm package name behind the module ids of a build — the set that must be covered. */
function bundledPackages(modules: string[]): string[] {
    const names = new Set<string>();
    for (const id of modules) {
        const marker = id.replace(/\\/g, "/").lastIndexOf("/node_modules/");
        if (marker < 0) continue;
        const rest = id.slice(marker + "/node_modules/".length).split("/");
        names.add(rest[0].startsWith("@") ? `${rest[0]}/${rest[1]}` : rest[0]);
    }
    return [...names].sort();
}

describe("third-party licences", () => {
    it("ships a licence text for every package in the web bundle", async () => {
        const built = await web();
        const notices = noticesOf(built);
        const modules = modulesOf(built);
        // A build that emitted nothing would satisfy every assertion below.
        expect(modules.some((id) => id.endsWith("/src/lib/platform/web.ts"))).toBe(true);

        const packages = bundledPackages(modules);
        expect(packages).toContain("leaflet"); // the map, a real dependency
        expect(packages).toContain("svelte"); // …and the devDependency whose runtime ships

        const uncovered = packages.filter((name) => !notices.includes(`\n${name} `));
        expect(uncovered).toEqual([]);

        // The permission text itself has to be there, which is the point of the file.
        expect(notices).toContain("Permission is hereby granted, free of charge"); // MIT
        expect(notices).toContain("Redistribution and use in source and binary forms"); // BSD-2
        // Our own terms and the map data's, so a reader knows what the bundle itself is.
        expect(notices).toContain("GPL-3.0");
        expect(notices).toContain(`Map data ${registryAttribution("osm-planet")}, under the ODbL`);
    }, 180_000);

});

// The static web tier is published under a project sub-path and is expected to
// move to its own domain. Nothing in the built bundle may assume either one:
//
//   * asset URLs must be relative — a bundle with `/assets/…` in its HTML is
//     mounted-at-root-only and would 404 under any prefix.
//   * built the way the deploy builds it, no absolute site URL survives. A
//     hard-coded github.io URL would keep every link working until the move.
function offenders(files: Map<string, string>, needle: string | RegExp): string[] {
    const hit =
        typeof needle === "string"
            ? (t: string) => t.includes(needle)
            : (t: string) => needle.test(t);
    return [...files].filter(([, text]) => hit(text)).map(([name]) => name);
}

/**
 * A path-absolute reference to the project-Pages sub-path, which is what breaks
 * on a move. Deliberately not a bare substring search: a link to the repository
 * contains the same characters, and the preceding quote or bracket is what
 * distinguishes a path from part of some other URL.
 */
const ROOTED_SITE_PATH = /["'`(=,]\/OpenBikeComputer\//;

/**
 * The site's own origin, which a portable bundle must never contain.
 * `updates.openbikecomputer.com` is the firmware-update host: an endpoint the
 * app fetches a manifest from, not a self-reference, and moving the site does
 * not move it.
 */
const SITE_ORIGIN = /https?:\/\/(?:www\.)?openbikecomputer\.com/;

describe("deployment portability", () => {
    it("references its own assets relatively", async () => {
        const html = textFiles(await web()).get("index.html") ?? "";
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
        const files = textFiles(await web());
        expect(offenders(files, "timohueser.github.io")).toEqual([]);
        expect(offenders(files, SITE_ORIGIN)).toEqual([]);
        expect(offenders(files, ROOTED_SITE_PATH)).toEqual([]);
        // …and the carve-out is not vacuous: the update host really is in the bundle.
        expect(offenders(files, "https://updates.openbikecomputer.com/").length).toBeGreaterThan(0);
    }, 180_000);
});

// The USB stack must stay in its own chunk, not the web tier's entry bundle. `lib/device/session`
// reaches it through a dynamic `import()` so a visitor who only downloads a map never fetches the
// transport, the codecs and the client. That split disappears *silently*: everything still works,
// the entry chunk is just bigger.

const IS_USB = /\/src\/lib\/usb\//;

describe("the USB stack's chunk", () => {
    it("is code-split out of the web tier's entry bundle", async () => {
        const built = await web();

        // Guard the guard: if the glob stopped matching, every assertion below would pass vacuously.
        const usbModules = modulesOf(built).filter((id) => IS_USB.test(id));
        expect(usbModules.some((id) => id.endsWith("/src/lib/usb/client.ts"))).toBe(true);
        expect(usbModules.some((id) => id.endsWith("/src/lib/usb/webusb.ts"))).toBe(true);

        const inEntry = entryModulesOf(built).filter((id) => IS_USB.test(id));
        expect(inEntry, "the USB stack leaked into the entry chunk").toEqual([]);
    }, 180_000);

    it("does not ship the simulated device", async () => {
        // `loopback.ts` is the in-memory transport and `flat-device.ts` with its wasm package is a
        // whole device. Neither belongs in anything a person visits.
        const built = await web();
        const shipped = modulesOf(built).filter((id) =>
            /\/src\/lib\/usb\/(loopback|flat-device)\.ts$|\/test-support\/flat-device\//.test(id),
        );
        expect(shipped).toEqual([]);
        const wasm = built.filter((item) => item.type === "asset" && item.fileName.endsWith(".wasm"));
        expect(wasm).toHaveLength(1);
        const source = wasm[0].source;
        expect(source).toBeInstanceOf(Uint8Array);
        const exports = WebAssembly.Module.exports(new WebAssembly.Module(source as Uint8Array<ArrayBuffer>));
        expect(exports.some((entry) => /flatdevice|devicereaction/.test(entry.name))).toBe(false);
    }, 180_000);
});
