// The third-party notice guard for #1149: whatever the bundler puts in the bundle, its
// licence text has to ship beside it.
//
// Same method as bundle.test.ts — build the real thing and inspect the emitted output —
// because the claim is about the build product. A list of `dependencies` would prove
// nothing here: Svelte is a devDependency whose runtime is compiled into every chunk, and
// `@tauri-apps/api` is a dependency the web tier never bundles.

import { build } from "vite";
import { describe, expect, it } from "vitest";

const FILE = "third-party-licenses.txt";

interface BuiltOutput {
    output: Array<{
        type: string;
        fileName?: string;
        source?: string | Uint8Array;
        modules?: Record<string, unknown>;
    }>;
}

/** The emitted notices plus the module ids of the same build, for one target. */
async function licensesFor(mode: string): Promise<{ notices: string; modules: string[] }> {
    const out = await build({
        mode,
        logLevel: "error",
        build: { write: false, outDir: "dist/.licenses-test" },
    });
    const result = (Array.isArray(out) ? out[0] : out) as unknown as BuiltOutput;
    const asset = result.output.find((c) => c.type === "asset" && c.fileName === FILE);
    const modules = result.output.flatMap((c) =>
        c.type === "chunk" ? Object.keys(c.modules ?? {}) : [],
    );
    return { notices: String(asset?.source ?? ""), modules };
}

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
        const { notices, modules } = await licensesFor("web");
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
        const desktop = await licensesFor("desktop");
        expect(bundledPackages(desktop.modules)).toContain("@tauri-apps/api");
        expect(desktop.notices).toContain("@tauri-apps/api");
    }, 180_000);
});
