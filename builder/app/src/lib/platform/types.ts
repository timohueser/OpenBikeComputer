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
//     member would be, so reaching for it is a type error rather than a dead
//     runtime control.
//   * **Not written yet.** A capability the tier is *meant* to have but whose
//     implementation lands in a later sub-issue throws
//     `PlatformNotImplemented`, naming the issue that fills it in. Loud,
//     greppable, and impossible to mistake for a product decision.
//
// Capability flags are the only gating input the UI is allowed to read. There
// is deliberately no `isWeb`.

import type { Component } from "svelte";
import type { Preset, SchemaEnvelope } from "../config/model";
import type { DeviceSession } from "../usb/session";
import type { RideLibrary } from "../device/library";

export type PlatformName = "web" | "desktop" | "dev";

/** The device's color gamut, laid out for the picker grid. */
export interface Palette {
    columns: number;
    colors: string[];
}

/** The localhost-only source state for the maintainer schema lab. */
export interface SchemaPreviewStatus {
    available: boolean;
    label: string;
    configured: boolean;
    detail: string;
    bbox: string;
}

/** One native crop pack. Rendering remains in the production wasm renderer. */
export interface SchemaPreviewMap {
    bytes: Uint8Array;
    packDurationMs: number;
}

export interface SchemaPreviewService {
    status(): Promise<SchemaPreviewStatus>;
    pack(config: Record<string, unknown>, signal: AbortSignal): Promise<SchemaPreviewMap>;
}

/**
 * What the UI knows how to ask for. Every flag is a statement about the *tier*,
 * not about how far its implementation has got: `deviceUsb` is true on the web
 * host because WebUSB is that tier's design, even though C3 (#902) is what
 * makes the call work. C2 (#901) turns these into inline disabled affordances.
 */
export interface Caps {
    /** Keeps pulled rides in a managed folder the user can see and back up. */
    readonly rideLibrary: boolean;
    /** Can reach a device over USB at all. */
    readonly deviceUsb: boolean;
    /** Shows the device dashboard and the schema-driven settings editor. */
    readonly deviceDashboard: boolean;
}

// --- seams whose payloads are not designed yet -------------------------------
//
// These are real seams with no backing implementation anywhere yet. The honest
// thing is to name them and leave the payload opaque: guessing a shape now buys
// nothing and costs a breaking change when the issue that owns it lands. Each
// alias is one line to fill in — `DeviceSession` below was one of them until
// C3 (#902) defined it.

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

/** One verified assembly's native output folder. Browser hosts use their own
 * downloader and therefore have no session. */
export interface MapOutputSession {
    readonly path: string;
    write(name: string, bytes: Uint8Array): Promise<string>;
    finish(): Promise<void>;
}

// --- the style editor, as a lazily loaded module -----------------------------

/**
 * The style editor route, as `import()` hands it back. This is a type-only
 * reference, erased before the bundler sees it — naming the route here does
 * not pull it into any bundle.
 */
export type StyleEditorModule = { default: Component<Record<string, never>> };

/** Loads the maintainer-only style editor. A product host has no `import()` of
 *  the route anywhere in its graph, so
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

    /** The shipped schema presets, used only by the maintainer editor. */
    presets(): Promise<Preset[]>;
    /**
     * The cell catalog root exactly as fetched: resolved URL plus raw body. The
     * catalog client owns validation so every host exposes one root-fetch seam.
     */
    catalog(): Promise<{ url: string; body: string }>;
    /** Transport for digest-pinned catalog satellites and cells. The web/dev
     *  hosts use fetch; desktop uses its same-origin native command. */
    readonly catalogFetch: typeof fetch;
    /** Open one grouped native output. Null on browser hosts. */
    readonly openMapOutput: ((name: string) => Promise<MapOutputSession>) | null;

    /** Non-null exactly when `caps.deviceUsb`. */
    readonly device: (() => Promise<DeviceSession>) | null;
    /** Non-null exactly when `caps.rideLibrary`. */
    readonly rides: (() => Promise<RideLibrary>) | null;

    /**
     * obc-pack's config JSON Schema envelope for the maintainer editor. Non-null
     * exactly when this host exports `loadStyleEditor`.
     */
    readonly schema: (() => Promise<SchemaEnvelope>) | null;

    /** The device's color gamut for the maintainer editor's picker grid. */
    readonly palette: (() => Promise<Palette>) | null;

    /** Native fixed-crop packer behind the maintainer-only Advanced route.
     * Null in both product hosts: neither accepts local PBF/schema input. */
    readonly schemaPreview: SchemaPreviewService | null;

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

    /** Show a managed file in the OS file manager. */
    readonly revealFile?: (path: string) => Promise<void>;

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
