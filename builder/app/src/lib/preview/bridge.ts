/**
 * The browser side of the preset-preview renderer (epic #894, B2 — issue #899).
 *
 * The hosted tier has no style editor, so presets are the only styling a visitor sees and each one
 * has to show what it draws. `apps/obc-web-preview` compiled to wasm does that: a small demo
 * `.obcm`, baked from that preset's own config by `builder/bake-previews.sh`, rendered at the
 * panel's own 240×320 by the exact `obc-reader` + `obc-render` code the nRF54L runs. No screenshot
 * pipeline anyone has to remember to re-run, and no mockup.
 *
 * **Everything here is behind a dynamic `import()`**, and it matters more than it does for the
 * conversion bridge: the module is ~60 kB gzipped and each demo map is 70–800 kB, so a visitor who
 * never scrolls to the styles must pay for none of it. `preview/bundle.test.ts` asserts that
 * against the chunks Rollup actually emits.
 *
 * Failures are {@link PreviewError}s. None of them are a visitor's fault — a preview map is an
 * artifact this repo committed — so the messages are written for whoever deployed the site.
 */

import type { InitInput } from "./pkg/obc_web_preview.js";

/**
 * Why a preview failed. Mirrors `PreviewErrorCode::as_str` in
 * `apps/obc-web-preview/src/preview.rs` — the two are one contract, so add or rename in both.
 *
 * - `not-a-map` — the bytes are not an OBCM map, or not a version this build reads (a demo map
 *   left un-rebaked across an OBCM bump).
 * - `truncated` — the fetch returned a short body.
 * - `internal` — a defect in the bridge, or the module or map failed to load. The message says so.
 */
export type PreviewErrorCode = "not-a-map" | "truncated" | "internal";

/** A preview failure: a stable {@link PreviewErrorCode} plus a message. */
export class PreviewError extends Error {
    readonly code: PreviewErrorCode;

    constructor(code: PreviewErrorCode, message: string) {
        super(message);
        this.name = "PreviewError";
        this.code = code;
    }
}

type Bridge = typeof import("./pkg/obc_web_preview.js");

/** One open demo map with a camera over it — the wasm object, narrowed to what the UI uses. */
export interface Preview {
    readonly width: number;
    readonly height: number;
    /** RGBA view over wasm memory. Blit it immediately; any later call may detach it. */
    frame(): Uint8ClampedArray;
    /** Drag by a screen-space delta in pixels. */
    pan(dx: number, dy: number): void;
    /** Scale the zoom (`>1` zooms in), clamped in Rust. */
    zoom_by(factor: number): void;
    /** Frame an explicit bbox in microdegrees, and go there. */
    fit_bbox(minLon: number, minLat: number, maxLon: number, maxLat: number): void;
    /** Back to the opening view. */
    reset(): void;
    /** Whether the next `frame()` will redraw. */
    is_dirty(): boolean;
    /** Ground metres per pixel at the current zoom. */
    meters_per_pixel(): number;
    /** Release the ≈370 kB of reader cache and render scratch. Nothing else may be called after. */
    free(): void;
}

/**
 * The in-flight or settled module load. Memoized so several cards share one fetch; cleared on
 * failure so a transient network error can be retried rather than cached forever.
 */
let loading: Promise<Bridge> | null = null;

/**
 * Load and instantiate the wasm module, if it is not already up.
 *
 * `source` overrides where the `.wasm` comes from. Leave it out in the browser: the generated glue
 * resolves the module next to itself, which is the form the bundler rewrites to a hashed asset
 * URL. Node has no `fetch` for `file:` URLs, so tests pass the bytes directly.
 *
 * Calling this when the styles section comes into view turns the first render into a plain
 * function call. It is optional; {@link openPreview} loads on demand.
 */
export function initPreview(source?: InitInput): Promise<void> {
    if (!loading) {
        const pending = load(source);
        loading = pending;
        // Drop the memo if it settles as a failure, so the next call retries. Attached here (not
        // in the caller) so a caller that ignores the returned promise still cannot wedge the
        // module into a permanently-failed state.
        pending.catch(() => {
            if (loading === pending) loading = null;
        });
    }
    return loading.then(() => undefined);
}

async function load(source?: InitInput): Promise<Bridge> {
    let mod: Bridge;
    try {
        mod = await import("./pkg/obc_web_preview.js");
        await mod.default(source === undefined ? undefined : { module_or_path: source });
    } catch (cause) {
        throw new PreviewError(
            "internal",
            `The preview renderer could not be loaded (${describe(cause)}). Check your connection and reload the page.`,
        );
    }
    return mod;
}

/**
 * Open a demo map's bytes for rendering.
 *
 * The returned object owns wasm memory — call {@link Preview.free} when the card goes away, since
 * wasm-bindgen has no GC hook to do it for you.
 *
 * @throws {PreviewError} — see {@link PreviewErrorCode}.
 */
export async function openPreview(map: Uint8Array): Promise<Preview> {
    const mod = await ensure();
    try {
        return new mod.MapPreview(map) as unknown as Preview;
    } catch (cause) {
        throw asPreviewError(cause);
    }
}

function ensure(): Promise<Bridge> {
    initPreview();
    // `initPreview` always assigns before returning; the assertion just tells TypeScript so.
    return loading as Promise<Bridge>;
}

const CODES: ReadonlySet<string> = new Set<PreviewErrorCode>(["not-a-map", "truncated", "internal"]);

/**
 * Normalize whatever crossed the wasm boundary into a {@link PreviewError}. A value without a
 * known code is a trap, an out-of-memory, or a bug — reported as `internal` rather than passed
 * through, so callers only ever handle one error type.
 */
function asPreviewError(cause: unknown): PreviewError {
    if (cause instanceof PreviewError) return cause;
    if (typeof cause === "object" && cause !== null) {
        const { code, message } = cause as { code?: unknown; message?: unknown };
        if (typeof code === "string" && CODES.has(code) && typeof message === "string") {
            return new PreviewError(code as PreviewErrorCode, message);
        }
    }
    return new PreviewError("internal", `The preview failed unexpectedly (${describe(cause)}).`);
}

function describe(cause: unknown): string {
    if (cause instanceof Error) return cause.message;
    return String(cause);
}
