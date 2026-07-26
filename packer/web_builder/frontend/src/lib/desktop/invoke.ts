// The desktop host's transport: Tauri commands. Only platform/desktop.ts and
// the desktop build tracker import it, so it never reaches the other two
// bundles — the same containment `api/client.ts` has on the dev side.
//
// Every function here is one `invoke()`. The names are the Rust command names in
// firmware/obc-desktop/src/main.rs, and the argument shapes are what serde
// deserializes there; that is the whole contract, and it is worth keeping in one
// file so a rename is one place on each side.

import { invoke } from "@tauri-apps/api/core";
import type { Palette, RegionFeature, StoragePlace } from "../platform/types";
import type { Preset, SchemaEnvelope } from "../config/model";

/** The catalog manifest, plus the URL it came from — §2 resolves preview
 *  references against the manifest's own location. */
export interface FetchedCatalog {
    url: string;
    body: string;
}

/** `build_active`'s answer: the running (or most recent) build. */
export interface JobSnapshot {
    id: string;
    state: "running" | "done" | "error" | "cancelled";
}

/** What a build reports. Mirrors the dev server's SSE events, deliberately —
 *  see `lib/build/phases.ts`. */
export type BuildEvent =
    | { type: "status"; status: string; detail: string }
    | { type: "progress"; phase: string; region: string; pct: number }
    | { type: "log"; line: string }
    | { type: "done"; path: string; filename: string; size: number }
    | { type: "error"; message: string }
    | { type: "cancelled" };

export const desktop = {
    regions: () => invoke<{ features: RegionFeature[] }>("regions").then((fc) => fc.features),
    presets: () => invoke<Preset[]>("presets"),
    schema: () => invoke<SchemaEnvelope>("schema"),
    palette: () => invoke<Palette>("palette"),
    catalog: () => invoke<FetchedCatalog>("catalog"),

    buildActive: () => invoke<JobSnapshot | null>("build_active"),
    buildCancel: (id: string) => invoke<boolean>("build_cancel", { id }),

    storagePlaces: () => invoke<StoragePlace[]>("storage_info"),
    storageClear: (id: string) => invoke<number>("storage_clear", { id }),

    revealFile: (path: string) => invoke<void>("reveal_file", { path }),
};
