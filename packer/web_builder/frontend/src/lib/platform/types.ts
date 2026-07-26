// The seam between this app and whatever is hosting it (issue #895). Three
// hosts exist — the static hosted site, the desktop app, and today's local
// FastAPI dev server — and they differ in what they can *do*, not in how the
// app is written. Which one is compiled in is a build-time decision made by an
// alias in vite.config.ts, so the two hosts you didn't build never enter the
// module graph.
//
// Two different kinds of "not here" live in this file, and keeping them apart
// is the point of the shapes below:
//
//   * **Absent by design.** A host without a capability has `null` where the
//     member would be. `caps.build === false` and `buildMap === null` are the
//     same fact, so starting a build on a host that cannot run one is a type
//     error rather than a runtime one — there is no call to write.
//   * **Not written yet.** A capability the tier is *meant* to have but whose
//     implementation lands in a later sub-issue throws
//     `PlatformNotImplemented`, naming the issue that fills it in. Loud,
//     greppable, and impossible to mistake for a product decision.
//
// Capability flags are the only gating input the UI is allowed to read. There
// is deliberately no `isWeb`.

import type { Component } from "svelte";
import type { Preset, SchemaEnvelope } from "../config/model";

export type PlatformName = "web" | "desktop" | "dev";

/** One Geofabrik download region: the picker's unit of selection. */
export interface RegionFeature {
    type: "Feature";
    properties: { id: string; name: string; parent: string | null; has_children: boolean };
    geometry: { type: "Polygon" | "MultiPolygon"; coordinates: unknown };
}

/** The device's color gamut, laid out for the picker grid. */
export interface Palette {
    columns: number;
    colors: string[];
}

/**
 * What the UI knows how to ask for. Every flag is a statement about the *tier*,
 * not about how far its implementation has got: `deviceUsb` is true on the web
 * host because WebUSB is that tier's design, even though C3 (#902) is what
 * makes the call work. C2 (#901) turns these into inline disabled affordances.
 */
export interface Caps {
    /** Runs obc-pack locally, so a map can be built from raw OSM extracts. */
    readonly build: boolean;
    /** Crops a build to a drawn box; whole regions only when false. */
    readonly bboxCrop: boolean;
    /** Ships the advanced style editor (`components/advanced/`). */
    readonly styleEditor: boolean;
    /** Keeps pulled rides in a managed folder the user can see and back up. */
    readonly rideLibrary: boolean;
    /** Can reach a device over USB at all. */
    readonly deviceUsb: boolean;
    /** Shows the device dashboard and the schema-driven settings editor. */
    readonly deviceDashboard: boolean;
}

// --- building ---------------------------------------------------------------

/** A map build, in the app's own terms. Hosts translate to their transport. */
export interface BuildRequest {
    regionIds: string[];
    config: unknown;
    chunkSize?: number;
    outputName: string;
    /** [west, south, east, north] degrees; only hosts with `caps.bboxCrop`. */
    bbox?: [number, number, number, number];
}

export interface BuildResult {
    downloadUrl: string;
    filename: string;
    size: number;
}

export type BuildState = "idle" | "starting" | "running" | "done" | "error";

/**
 * One build, followed to completion. The fields are read reactively by the UI,
 * so an implementation makes them `$state` — the interface only promises they
 * are observable, not how (the dev host polls an SSE log; D1's desktop host
 * will bridge Tauri events into the same shape).
 */
export interface BuildSession {
    readonly state: BuildState;
    readonly phase: string;
    readonly pct: number;
    readonly logLines: readonly string[];
    readonly transientLine: string | null;
    readonly result: BuildResult | null;
    readonly error: string | null;
    start(req: BuildRequest): Promise<void>;
    /** Re-attach to a build started before a page reload; false if none. */
    reattach(): Promise<boolean>;
}

/** Opens a build session. Present only on a host with `caps.build`. */
export type StartBuild = () => BuildSession;

// --- seams whose payloads are not designed yet -------------------------------
//
// These three are real seams with no backing implementation anywhere yet. The
// honest thing is to name them and leave the payload opaque: guessing a shape
// now buys nothing and costs a breaking change when the issue that owns it
// lands. Each alias is one line to fill in.

/**
 * The catalog of pre-baked maps. A3 (#897) owns the manifest format and the
 * version law that binds it to OBCM; C1 (#900) is its first consumer.
 *
 * Not to be confused with `public/osm_catalog.json`, which is the OSM tag-key
 * catalog the style editor's category rail reads.
 */
export type MapCatalog = unknown;

/** A connected device. C3 (#902) defines the protocol client and the WebUSB
 *  byte pipe; D4 (#909) the native `nusb` one. */
export type DeviceSession = unknown;

/** The managed ride library. E2 (#912) owns it. */
export type RideLibrary = unknown;

// --- the style editor, as a lazily loaded module -----------------------------

/**
 * The style editor route, as `import()` hands it back. This is a type-only
 * reference, erased before the bundler sees it — naming the route here does
 * not pull it into any bundle.
 */
export type StyleEditorModule = { default: Component<Record<string, never>> };

/** Loads the style editor. Present only on a host with `caps.styleEditor`; a
 *  host that lacks it has no `import()` of the route anywhere in its graph, so
 *  the chunk cannot be emitted rather than merely going unused. */
export type LoadStyleEditor = () => Promise<StyleEditorModule>;

// --- the interface -----------------------------------------------------------

export interface Platform {
    /** Diagnostics and error messages only — never branch on it. Gate on
     *  `caps`, which is the tier's contract; this is just its label. */
    readonly name: PlatformName;
    readonly caps: Caps;

    /** Geofabrik's download-region tree, for the area picker. */
    regions(): Promise<RegionFeature[]>;
    /** The shipped style presets, default first. */
    presets(): Promise<Preset[]>;
    /** obc-pack's config JSON Schema envelope — the editor's capability probe. */
    schema(): Promise<SchemaEnvelope>;
    /** The device's color gamut for the picker grid. */
    palette(): Promise<Palette>;
    /** The pre-baked map catalog (see `MapCatalog`). */
    catalog(): Promise<MapCatalog>;

    /** Non-null exactly when `caps.build`. */
    readonly buildMap: StartBuild | null;
    /** Non-null exactly when `caps.deviceUsb`. */
    readonly device: (() => Promise<DeviceSession>) | null;
    /** Non-null exactly when `caps.rideLibrary`. */
    readonly rides: (() => Promise<RideLibrary>) | null;

    /**
     * The retired editor's server-side `user_config.json`, offered once for
     * import. Optional rather than a capability because only the FastAPI dev
     * host ever had server-side config to migrate — there is nothing for the
     * other two tiers to implement, now or later.
     */
    readonly legacyConfig?: () => Promise<Record<string, unknown> | null>;
}

/**
 * A seam that this tier is meant to have but that hasn't been implemented yet.
 * Distinct from a `null` member, which means "this tier will never have it".
 * `owner` names the sub-issue that fills it in, so the stack trace says who to
 * chase.
 */
export class PlatformNotImplemented extends Error {
    constructor(host: PlatformName, member: string, owner: string) {
        super(`${host} platform: ${member}() is not implemented yet — lands in ${owner}`);
        this.name = "PlatformNotImplemented";
    }
}
