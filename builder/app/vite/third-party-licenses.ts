// Third-party licence notices for the shipped web bundle (#1149).
//
// MIT, BSD and friends all say the same thing: the copyright notice and the permission
// text have to travel with the distribution. A `dependencies` list cannot answer that
// question — Svelte is a *devDependency* whose runtime is nonetheless compiled into every
// chunk, and plenty of production deps are tree-shaken away entirely. The only honest
// source is the build product itself, so this reads the modules Rollup actually put in the
// emitted chunks and collects a licence text for each npm package behind them.
//
// The result is emitted as an asset, which means it ships with the static site *and* inside
// the Tauri bundle, because both are the same `vite build`. The Rust side of the desktop app
// and the firmware are a separate obligation, covered by the repo's THIRD-PARTY.md.

import fs from "node:fs";
import path from "node:path";
import type { Plugin } from "vite";

// LICENSE, LICENCE, LICENSE.md, COPYING, NOTICE — and the dual-licensed shape, where a
// package ships one file per option (`LICENSE_MIT` + `LICENSE_APACHE-2.0`, @tauri-apps/api).
// Both are collected: the licence is the package's to choose between, not ours to narrow.
// The optional tail must start with a separator so `licenses.js` is not a licence file.
const LICENSE_FILE = /^(licen[cs]e|copying|notice)([._-][\w.+-]+)*$/i;

interface PackageNotice {
    name: string;
    version: string;
    license: string;
    url: string;
    text: string;
}

/** The npm package directory an absolute module id belongs to, or null for our own source. */
function packageRootOf(id: string): string | null {
    const clean = id.split("?")[0].replace(/\\/g, "/");
    const marker = clean.lastIndexOf("/node_modules/");
    if (marker < 0) return null;
    const rest = clean.slice(marker + "/node_modules/".length).split("/");
    // A scoped package is two segments; anything else is one.
    const depth = rest[0].startsWith("@") ? 2 : 1;
    if (rest.length < depth) return null;
    return clean.slice(0, marker + "/node_modules/".length) + rest.slice(0, depth).join("/");
}

function licenseTextIn(dir: string): string | null {
    let entries: string[];
    try {
        entries = fs.readdirSync(dir);
    } catch {
        return null;
    }
    const names = entries.filter((e) => LICENSE_FILE.test(e)).sort();
    if (!names.length) return null;
    const texts = names
        .map((n) => {
            try {
                const p = path.join(dir, n);
                if (!fs.statSync(p).isFile()) return "";
                const body = fs.readFileSync(p, "utf8").trim();
                // Name the file when a package ships several: an Apache text and an MIT text
                // run together read as one confused licence.
                return body && names.length > 1 ? `--- ${n} ---\n\n${body}` : body;
            } catch {
                return "";
            }
        })
        .filter(Boolean);
    return texts.length ? texts.join("\n\n") : null;
}

/** Whatever the package says about itself — `license`, or the legacy `licenses` array. */
function declaredLicense(pkg: Record<string, unknown>): string {
    if (typeof pkg.license === "string") return pkg.license;
    if (pkg.license && typeof pkg.license === "object") {
        const t = (pkg.license as { type?: string }).type;
        if (t) return t;
    }
    if (Array.isArray(pkg.licenses)) {
        const types = (pkg.licenses as Array<{ type?: string }>).map((l) => l.type).filter(Boolean);
        if (types.length) return types.join(" OR ");
    }
    return "";
}

function projectUrl(pkg: Record<string, unknown>): string {
    const repo = pkg.repository;
    const raw =
        typeof repo === "string"
            ? repo
            : repo && typeof repo === "object"
              ? ((repo as { url?: string }).url ?? "")
              : "";
    const url = raw || (typeof pkg.homepage === "string" ? pkg.homepage : "");
    return url.replace(/^git\+/, "").replace(/\.git$/, "").replace(/^git:\/\//, "https://");
}

function noticeFor(dir: string): PackageNotice {
    const manifest = path.join(dir, "package.json");
    const pkg = JSON.parse(fs.readFileSync(manifest, "utf8")) as Record<string, unknown>;
    const name = String(pkg.name ?? path.basename(dir));
    const text = licenseTextIn(dir);
    const license = declaredLicense(pkg);
    if (!text) {
        // Not a warning: a package whose licence text we cannot ship is a package we cannot
        // lawfully ship. Vendor the text or drop the dependency — do not paper over it.
        throw new Error(
            `third-party-licenses: ${name} declares "${license || "no licence"}" but ships no ` +
                `licence file in ${dir}. Its notice cannot travel with the bundle.`,
        );
    }
    return { name, version: String(pkg.version ?? "?"), license, url: projectUrl(pkg), text };
}

function render(notices: PackageNotice[]): string {
    const rule = "=".repeat(92);
    const head = [
        "OpenBikeComputer map builder — third-party licences",
        "",
        "This file lists every third-party package whose code is compiled into the bundle beside",
        "it, together with the licence text that package ships. It is generated from the modules",
        "the bundler actually emitted, not from a dependency list, so it describes this build and",
        "no other.",
        "",
        "The application's own source is GPL-3.0: https://github.com/timohueser/OpenBikeComputer",
        "Map data © OpenStreetMap contributors, under the ODbL: https://www.openstreetmap.org/copyright",
        "",
        `${notices.length} package(s):`,
        ...notices.map((n) => `  - ${n.name} ${n.version} — ${n.license || "see text"}`),
        "",
    ];
    const bodies = notices.map((n) =>
        [
            rule,
            `${n.name} ${n.version}${n.license ? ` — ${n.license}` : ""}`,
            ...(n.url ? [n.url] : []),
            rule,
            "",
            n.text,
            "",
        ].join("\n"),
    );
    return [...head, ...bodies].join("\n");
}

/**
 * Emit `third-party-licenses.txt` next to the bundle it describes.
 *
 * Build-only by construction: there is no bundle to read in `vite dev`, and a stale file
 * would be worse than none.
 */
export function thirdPartyLicenses(fileName = "third-party-licenses.txt"): Plugin {
    return {
        name: "obc-third-party-licenses",
        apply: "build",
        generateBundle(_options, bundle) {
            const roots = new Set<string>();
            for (const chunk of Object.values(bundle)) {
                if (chunk.type !== "chunk") continue;
                for (const id of Object.keys(chunk.modules)) {
                    if (id.startsWith("\0")) continue; // rollup virtual module
                    const root = packageRootOf(id);
                    if (root) roots.add(root);
                }
            }
            const notices = [...roots]
                .map(noticeFor)
                .sort((a, b) => a.name.localeCompare(b.name, "en"));
            this.emitFile({ type: "asset", fileName, source: render(notices) });
        },
    };
}
