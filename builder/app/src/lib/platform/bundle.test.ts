// The build-product guard: every claim this repository makes about the files
// Vite actually emits is asserted here — the host split (#895), the third-party
// notices (#1149), deployment portability (#905) and the USB stack's chunk
// (#909). Each block asserts against what Rollup put in the emitted output, not
// against source imports or a grep of them.
//
// Those were four files with four build wrappers, ten Vite builds between them
// for four distinct products. Vitest isolates by file, not by test, so one file
// is the only unit of sharing it offers: they are one file now, and each
// product is built once. Four is the floor — `web`, `web` built the way the
// deploy builds it, `production` (the dev host) and `desktop` are four
// different build inputs and so four different products. Count the calls to
// {@link product} below; there are four.

import { build } from "vite";
import { describe, expect, it } from "vitest";

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
 * One build per distinct product, memoised by mode **and** env — and
 * **serialised**, which is the correctness half rather than the speed half.
 * Vite reads VITE_-prefixed variables off the one `process.env` this process
 * has, so a build that needs one sets it and restores it in a `finally`; two
 * builds awaited concurrently would read each other's. Every build is chained
 * onto a single queue, so no two ever overlap.
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

/** The static web tier as `obc web` and `npm run build:web` produce it, the same
 *  tier as `.github/workflows/deploy-site.yml` produces it, the maintainer dev
 *  host, and the Tauri app. */
const web = () => product("web");
const deployedWeb = () => product("web", { VITE_SITE_BASE: "../" });
const dev = () => product("production");
const desktop = () => product("desktop");

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

// The bundle-split guard for #895: the static web tier must not ship the
// FastAPI job-polling client or the desktop-only style editor.
//
// Both targets are built: if the web assertions ever pass because the glob
// stopped matching anything, the dev assertions fail in the same run.

/**
 * Source modules the static web tier must not contain, and why.
 *
 * The Tauri rows are not the same claim as the rest: `@tauri-apps/api` in a
 * static site's bundle would be dead weight *and* a lie about where the app
 * runs, but more usefully, its presence would mean the host alias picked the
 * wrong module — which the assertions below could not otherwise notice, because
 * a desktop host's methods all resolve and simply never work in a browser.
 * (`lib/desktop/release.ts` is deliberately absent from this list: the *web*
 * tier reads it, to decide whether the desktop app has a download link yet.)
 */
const DESKTOP_ONLY = {
    "the FastAPI client": /\/src\/lib\/api\/client\.ts$/,
    "the dev host": /\/src\/lib\/platform\/dev\.ts$/,
    "the desktop host": /\/src\/lib\/platform\/desktop\.ts$/,
    "the Tauri command bridge": /\/src\/lib\/desktop\/invoke\.ts$/,
    "the native USB transport": /\/src\/lib\/desktop\/usb\.ts$/,
    "the native USB session": /\/src\/lib\/desktop\/usb\.svelte\.ts$/,
    "the ride library's Tauri backing": /\/src\/lib\/desktop\/library\.ts$/,
    "the Tauri JS API": /\/@tauri-apps\/api\//,
    "the style editor route": /\/src\/routes\/Advanced\.svelte/,
    "the style editor components": /\/src\/components\/advanced\//,
};

/** …and the ones only the *desktop* target may contain, for the same reason in
 *  the other direction: a desktop build that quietly fell back to the dev host
 *  would talk to a FastAPI server that isn't running. */
const DESKTOP_TARGET_ONLY = {
    "the desktop host": /\/src\/lib\/platform\/desktop\.ts$/,
    "the Tauri command bridge": /\/src\/lib\/desktop\/invoke\.ts$/,
    // D4 (#909): the desktop tier drives USB natively, so the Rust-backed byte
    // pipe must be in this bundle and nowhere else. Its presence here is also
    // what proves the dynamic `import()` in `desktop.ts` still resolves —
    // `device()` is only ever called from a click, so a broken path would
    // otherwise surface on someone's desk.
    "the native USB transport": /\/src\/lib\/desktop\/usb\.ts$/,
    "the native USB session": /\/src\/lib\/desktop\/usb\.svelte\.ts$/,
    // E2 (#912): the ride library is a folder on a disk, so only the tier with
    // one may carry the code that writes it. Its presence here is also what
    // proves `desktop.ts`'s dynamic `import()` still resolves — `rides()` is
    // only called from a click, so a broken path would surface on someone's
    // desk rather than in CI.
    "the ride library's Tauri backing": /\/src\/lib\/desktop\/library\.ts$/,
    "the Tauri JS API": /\/@tauri-apps\/api\//,
};

describe("bundle split", () => {
    it("keeps build and style-editor code out of the web target", async () => {
        const modules = modulesOf(await web());
        // An empty build, or one that quietly picked another host, would pass
        // every assertion below.
        expect(modules.some((id) => id.endsWith("/src/lib/platform/web.ts"))).toBe(true);
        expect(modules.some((id) => id.includes("/src/routes/Home.svelte"))).toBe(true);

        const found = Object.entries(DESKTOP_ONLY).flatMap(([what, re]) =>
            modules.filter((id) => re.test(id)).map((id) => `${what}: ${id}`),
        );
        expect(found).toEqual([]);

        // The cell composer is now the hosted builder rather than a lazily
        // loaded alternative selected by a manifest probe.
        expect(modules.some((id) => id.includes("/src/components/coverage/CoverageHome.svelte"))).toBe(
            true,
        );
    }, 180_000);

    it("ships the maintainer style editor only in the dev target", async () => {
        // The dev host's own set: everything in DESKTOP_ONLY except the three
        // rows that name the *desktop* host, which the dev target must not have
        // either — one alias picks exactly one host.
        const modules = modulesOf(await dev());
        const missing = Object.entries(DESKTOP_ONLY)
            .filter(([what]) => !(what in DESKTOP_TARGET_ONLY))
            .filter(([, re]) => !modules.some((id) => re.test(id)))
            .map(([what]) => what);
        expect(missing).toEqual([]);

        const strays = Object.entries(DESKTOP_TARGET_ONLY).flatMap(([what, re]) =>
            modules.filter((id) => re.test(id)).map((id) => `${what}: ${id}`),
        );
        expect(strays).toEqual([]);

        // `obc web` serves this target. Localhost is a secure WebUSB context,
        // so the build must retain the same lazy browser transport as the
        // static web host instead of rendering a desktop-only dead end.
        expect(modules.some((id) => id.endsWith("/src/lib/usb/webusb.ts"))).toBe(true);
    }, 180_000);

    it("wires the desktop target to the Tauri backend", async () => {
        const modules = modulesOf(await desktop());
        const missing = Object.entries(DESKTOP_TARGET_ONLY)
            .filter(([, re]) => !modules.some((id) => re.test(id)))
            .map(([what]) => what);
        expect(missing).toEqual([]);
        // …and not to the maintainer dev server.
        expect(modules.some((id) => /\/src\/lib\/api\/client\.ts$/.test(id))).toBe(false);
    }, 180_000);
});

// The third-party notice guard for #1149: whatever the bundler puts in the
// bundle, its licence text has to ship beside it.
//
// A list of `dependencies` would prove nothing here: Svelte is a devDependency
// whose runtime is compiled into every chunk, and `@tauri-apps/api` is a
// dependency the web tier never bundles.

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

        // Not just named — the permission text itself has to be there, which is the whole
        // point of the file. Two licences, two distinctive sentences.
        expect(notices).toContain("Permission is hereby granted, free of charge"); // MIT
        expect(notices).toContain("Redistribution and use in source and binary forms"); // BSD-2
        // Our own terms and the map data's, so a reader knows what the bundle itself is.
        expect(notices).toContain("GPL-3.0");
        expect(notices).toContain("OpenStreetMap contributors");
    }, 180_000);

    it("describes the tier it was built for, not a fixed list", async () => {
        // The desktop tier bundles the Tauri API the web tier does not, so the two files
        // must differ — a notice file that ignored the build product would be identical.
        const built = await desktop();
        expect(bundledPackages(modulesOf(built))).toContain("@tauri-apps/api");
        expect(noticesOf(built)).toContain("@tauri-apps/api");
    }, 180_000);
});

// The domain-move guard for C6 (#905): the static web tier is published today
// under GitHub Pages' project sub-path (`/OpenBikeComputer/builder/`) and is
// expected to move to its own domain. Nothing in the built bundle may assume
// either one. Two properties, and they fail differently:
//
//   * asset URLs are relative — a bundle with `/assets/…` in its HTML is
//     mounted-at-root-only and would 404 under any prefix. Vite's `base: "./"`
//     (#895) is what makes this true; this test is what keeps it true.
//   * built the way the deploy builds it, no absolute site URL survives
//     anywhere. The deploy passes VITE_SITE_BASE=../ because the docs and the
//     landing page are siblings in the same artifact; if a hard-coded github.io
//     URL crept back into a component, every link would keep working right up
//     until the move.

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

/**
 * The *site's own* origin — the thing a portable bundle must never contain, because the site is
 * what moves. An origin match rather than a bare `openbikecomputer.com` substring, for the same
 * reason {@link ROOTED_SITE_PATH} is not one: there is exactly one absolute URL this app is
 * *supposed* to know, and since #1002 it lives on a subdomain. `updates.openbikecomputer.com` is
 * the firmware-update host (#773) — a service endpoint the app fetches a manifest from, not a
 * self-reference. None of the three hosts is served from it, and moving the site does not move it.
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
        const files = textFiles(await deployedWeb());
        expect(offenders(files, "timohueser.github.io")).toEqual([]);
        expect(offenders(files, SITE_ORIGIN)).toEqual([]);
        expect(offenders(files, ROOTED_SITE_PATH)).toEqual([]);
        // …and the one carve-out above is not vacuous: the update host really is in the bundle,
        // so a rule written to permit it is being exercised rather than merely stated.
        expect(offenders(files, "https://updates.openbikecomputer.com/").length).toBeGreaterThan(0);
    }, 180_000);
});

// The USB stack must stay in its own chunk, not the web tier's entry bundle.
//
// `platform/web.ts` reaches it through a dynamic `import()` so a visitor who only downloads a map
// never fetches the transport, the codecs and the client — about 24 kB raw. That split is one
// ordinary-looking import away from disappearing, and it disappears *silently*: everything still
// works, the entry chunk is just bigger. So it is asserted against the chunks Rollup actually
// emitted, the same way A1 asserts the host split above.
//
// **The likeliest way to break it** is deduplication that looks like a tidy-up. C2's
// `platform/gating.ts` has its own one-line `hasWebUsb()` — `"usb" in navigator` — which overlaps
// this module's `webUsb()`. They are duplicated **on purpose**: `gating.ts` is imported by the home
// route and therefore lives in the entry chunk, so importing anything from `lib/usb/` into it would
// drag the whole stack in behind it. Two probes, two lines, no import edge. If you are here because
// you were about to merge them, this is the reason not to.

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

    it.each([
        ["web", web],
        ["desktop", desktop],
    ])("does not ship the simulated device (%s target)", async (_mode, target) => {
        // `loopback.ts` is a whole device — an object store, a catalog, id assignment. It exists so
        // the epic isn't blocked on #889's silicon, and it has no business in anything a person
        // installs or visits.
        //
        // The **desktop** row is not symmetry for its own sake. That app is the one people take to
        // a bench with a real board, and D4's (#909) on-glass recipe leans on "if the window says
        // Connected, something enumerated" as its first tell that a transfer is real. A simulated
        // device reachable from the shipped app would make that sentence false — quietly, and
        // exactly when someone is trying to decide whether hardware works.
        const shipped = modulesOf(await target()).filter((id) =>
            /\/src\/lib\/usb\/loopback\.ts$/.test(id),
        );
        expect(shipped).toEqual([]);
    }, 180_000);
});
