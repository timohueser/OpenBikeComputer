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
import type { Catalog } from "../catalog/manifest";
import type { Preset, SchemaEnvelope } from "../config/model";
import type { DeviceSession } from "../usb/session";
import type { RideLibrary } from "../device/library";

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
    /**
     * Where the map landed, when it landed somewhere real. Present only on a
     * host with a filesystem: the dev server keeps its output behind a download
     * URL, while the desktop app writes into a folder the user chose to have
     * and `revealFile` can open. Absent, not empty — "no path" and "the empty
     * path" must not be the same value.
     */
    path?: string;
}

/**
 * `cancelled` is its own state, not an error. A build the user stopped is the
 * thing they just asked for, and reporting it in red next to a stack of real
 * failures would be a lie about what happened.
 */
export type BuildState = "idle" | "starting" | "running" | "done" | "error" | "cancelled";

/**
 * One build, followed to completion. The fields are read reactively by the UI,
 * so an implementation makes them `$state` — the interface only promises they
 * are observable, not how (the dev host follows an SSE log; the desktop host
 * bridges a Tauri channel into the same shape).
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
    /**
     * Stop the running build, or `null` where a build cannot be stopped.
     *
     * Null on the dev host, and that is a statement about it rather than a gap:
     * it runs the packer as a subprocess behind an HTTP job queue with no
     * cancel endpoint, so there is nothing to call. The desktop host runs the
     * pack in-process against a cancel token that reaches inside the ingest and
     * simplify loops, so there it is a real button.
     */
    readonly cancel: (() => Promise<void>) | null;
}

/** Opens a build session. Present only on a host with `caps.build`. */
export type StartBuild = () => BuildSession;

// --- seams whose payloads are not designed yet -------------------------------
//
// These are real seams with no backing implementation anywhere yet. The honest
// thing is to name them and leave the payload opaque: guessing a shape now buys
// nothing and costs a breaking change when the issue that owns it lands. Each
// alias is one line to fill in — `DeviceSession` below was one of them until
// C3 (#902) defined it.

/**
 * The catalog of pre-baked maps: the OBCC manifest, whole and validated. A3
 * (#897) owns the format (`OBCC_Spec.md`) and the version law that binds it to
 * OBCM; a host implementing this method has already read one entire body and
 * parsed it as one document, because §7 admits nothing partial.
 *
 * Not to be confused with `public/osm_catalog.json`, which is the OSM tag-key
 * catalog the style editor's category rail reads.
 */
export type MapCatalog = Catalog;

/**
 * A device connection, followed over its lifetime — C3 (#902) defines it, over
 * the protocol client and the WebUSB byte pipe; D4 (#909) swaps in native
 * `nusb` underneath without changing this type.
 *
 * A *session*, not a device, because the browser forces it: WebUSB's chooser
 * only opens from a user gesture, so something observable has to exist before
 * any device is known — see `lib/usb/session.ts` for what that means for the UI.
 */
export type { DeviceSession };

/**
 * The managed ride library — a real folder, a small index, and the durable
 * write an `ackRides` is allowed to follow (E2 #912).
 *
 * The only tier that has one is the desktop app, and that is a statement about
 * durability rather than about effort: `synced` on the device means "a durable
 * copy of this ride exists off the device", and it is what unlocks deleting the
 * ride there and anchors its auto-expiry countdown (#638). A browser's OPFS is
 * evictable and its downloads are cancellable, so the hosted tier exports one
 * GPX, keeps no record, and never acks (`obc-ble-interface-spec.md` §4.4).
 */
export type { RideLibrary };

// --- what the app has put on this disk ---------------------------------------

/**
 * One directory the app writes to, as the UI shows it. `note` says what deleting
 * it costs, because the answer differs per place — re-downloading one region is
 * not the same sentence as re-downloading a 950 MB global dataset.
 */
export interface StoragePlace {
    readonly id: string;
    readonly label: string;
    readonly note: string;
    readonly path: string;
    readonly bytes: number;
    readonly files: number;
    readonly clearable: boolean;
}

/**
 * The caches, visible and clearable. Optional rather than a capability, for the
 * same reason `legacyConfig` is: a tier without a filesystem has nothing to
 * report and nothing to delete, so there is no gate to write and no issue that
 * owes it. The desktop app has it because its caches reach gigabytes and a user
 * is entitled to know where they are.
 */
export interface DiskStorage {
    places(): Promise<StoragePlace[]>;
    /** Delete one named place. Resolves to the bytes freed. */
    clear(id: string): Promise<number>;
}

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

    /**
     * True when `device()` runs on the browser's WebUSB, false when the host
     * drives USB itself. Deliberately *not* a `Caps` flag: every flag there
     * reads "this tier can do more when true", and this one is the opposite —
     * it says the tier borrows the browser's USB stack, and therefore inherits
     * the browser's answer.
     *
     * It exists because "does this tier do USB?" and "can this browser reach a
     * USB device?" are different questions with different remedies (#901).
     * `caps.deviceUsb` is true on the hosted site because WebUSB is that tier's
     * design — it is not a claim about Safari. The gating layer needs both
     * facts to pick the right sentence, and neither is derivable from the
     * other. Meaningless (and false) where `caps.deviceUsb` is false.
     */
    readonly usbViaWebUsb: boolean;

    /** Geofabrik's download-region tree, for the area picker. */
    regions(): Promise<RegionFeature[]>;
    /** The shipped style presets, default first. */
    presets(): Promise<Preset[]>;
    /** The pre-baked map catalog (see `MapCatalog`). */
    catalog(): Promise<MapCatalog>;

    /**
     * The catalog root exactly as fetched — resolved URL plus raw body — for
     * consumers that must see the document before an envelope is chosen.
     *
     * This is the seam envelope detection (#1038) stands on: one hosted URL
     * serves either a v1 manifest or a v2 cell-catalog root, the app peeks at
     * `schema_version` and commits to the matching flow, and the body it peeked
     * at is the body that flow parses — never a second fetch of a document that
     * could have changed in between. Implementations share the fetch with
     * `catalog()`, so a v1 root costs one request however it is read.
     *
     * Optional because only tiers whose catalog arrives as a fetched document
     * have anything to hand over; the dev host builds maps and has no catalog
     * at all (its `catalog()` is a named not-implemented, not a document).
     */
    readonly catalogRoot?: () => Promise<{ url: string; body: string }>;

    /** Non-null exactly when `caps.build`. */
    readonly buildMap: StartBuild | null;
    /** Non-null exactly when `caps.deviceUsb`. */
    readonly device: (() => Promise<DeviceSession>) | null;
    /** Non-null exactly when `caps.rideLibrary`. */
    readonly rides: (() => Promise<RideLibrary>) | null;

    /**
     * obc-pack's config JSON Schema envelope — what the editor and the build
     * card derive their capability from.
     *
     * The only member whose gate is a disjunction: non-null exactly when
     * `caps.build || caps.styleEditor`, its two callers. A tier with neither
     * has no config to validate and no packer to validate it against, so there
     * is nothing for it to serve — which is a permanent fact about that tier,
     * not a seam anyone is going to fill in. Hence `null`, like the rest.
     */
    readonly schema: (() => Promise<SchemaEnvelope>) | null;

    /** The device's color gamut for the picker grid. Non-null exactly when
     *  `caps.styleEditor`: the color picker is its only caller, and it lives
     *  inside the editor's code-split chunk. */
    readonly palette: (() => Promise<Palette>) | null;

    /**
     * The retired editor's server-side `user_config.json`, offered once for
     * import. Optional rather than a capability because only the FastAPI dev
     * host ever had server-side config to migrate — there is nothing for the
     * other two tiers to implement, now or later.
     */
    readonly legacyConfig?: () => Promise<Record<string, unknown> | null>;

    /** The app's own caches, where they are and how to get the space back.
     *  Optional for the same reason as `legacyConfig` — see [`DiskStorage`]. */
    readonly storage?: DiskStorage;

    /**
     * Show a produced file in the OS file manager. Present only where a build
     * result has a `path` at all: this is the desktop app's answer to the
     * hosted tier's download link, and a browser has no file manager to point
     * at.
     */
    readonly revealFile?: (path: string) => Promise<void>;

    /**
     * Save a small text document — a style export — and resolve with where it
     * went (E3 #913).
     *
     * Optional rather than a capability, and not because the tiers disagree
     * about *whether* you can export a style: all three can. They disagree
     * about **how a file leaves the app**. A browser has `<a download>`, which
     * is the right answer there and needs nothing from the host. Inside the
     * Tauri webview that same anchor does nothing at all — wry installs a
     * download delegate only when the embedder asks for one — so the desktop
     * app has to write the file itself, through a command that decides the
     * folder. Present exactly where the fallback does not work.
     */
    readonly saveText?: (name: string, text: string) => Promise<string>;

    /**
     * The site's outbound chrome — docs, the landing page's simulator, GitHub.
     *
     * Present only where the app is a page *of* the site; the desktop app is a
     * standalone window with no site around it, so there it is absent and the
     * header shows tabs instead. Optional rather than a capability for the
     * documented reason nav links are never gated (#901): a link has no moment
     * of intent, so there is no reason sentence to write and nothing for the
     * desktop page to list as an "add".
     */
    readonly siteNav?: { readonly docs: string; readonly simulator: string; readonly github: string };
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
