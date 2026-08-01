/**
 * Lazy browser bridge for #1045's live skin preview.
 *
 * The OBCM is the bakery's canonical Teningen fixture, emitted by Vite as a
 * separate asset rather than copied into the JS or wasm module. Opening the
 * editor is the only thing that fetches it and the renderer bridge; ordinary
 * skin picking continues to use the small digest-pinned PNGs from R2.
 */

import type { InitInput } from "./pkg/obc_skin_preview.js";
import type { SkinEntry } from "../catalog/manifest";

const MAP_URL = new URL("../../../../../host/obc-bake/assets/teningen-preview.obcm", import.meta.url);

type Bridge = typeof import("./pkg/obc_skin_preview.js");
type WasmPreview = InstanceType<Bridge["SkinPreview"]>;

export interface LiveSkinPreview {
    readonly width: number;
    readonly height: number;
    setSkin(skinJson: string): void;
    panBy(dx: number, dy: number): void;
    zoomAt(factor: number, x: number, y: number): void;
    resetCamera(): void;
    stats(): LivePreviewStats;
    frame(): Uint8ClampedArray;
    free(): void;
}

export interface LivePreviewStats {
    metersPerPixel: number;
    lodIndex: number;
    lodCount: number;
    featuresDrawn: number;
    featuresDropped: number;
    pointsDrawn: number;
    spanUtilization: number;
    pointUtilization: number;
    ringUtilization: number;
}

export interface SkinPreviewFrame {
    readonly width: number;
    readonly height: number;
    readonly pixels: Uint8ClampedArray;
}

interface ThumbnailOptions {
    open?: typeof openLiveSkinPreview;
    signal?: AbortSignal;
    yieldToBrowser?: () => Promise<void>;
}

let loading: Promise<Bridge> | null = null;
let mapLoading: Promise<Uint8Array> | null = null;

function describe(cause: unknown): string {
    return cause instanceof Error ? cause.message : String(cause);
}

async function module(source?: InitInput): Promise<Bridge> {
    if (!loading) {
        const pending = (async () => {
            const mod = await import("./pkg/obc_skin_preview.js");
            await mod.default(source === undefined ? undefined : { module_or_path: source });
            return mod;
        })();
        loading = pending;
        pending.catch(() => {
            if (loading === pending) loading = null;
        });
    }
    return loading;
}

async function mapBytes(fetchImpl: typeof fetch): Promise<Uint8Array> {
    if (!mapLoading) {
        const pending = (async () => {
            const response = await fetchImpl(MAP_URL);
            if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
            return new Uint8Array(await response.arrayBuffer());
        })();
        mapLoading = pending;
        pending.catch(() => {
            if (mapLoading === pending) mapLoading = null;
        });
    }
    return mapLoading;
}

export async function openLiveSkinPreview(
    schemaJson: string,
    skinJson: string,
    options: { fetchImpl?: typeof fetch; wasm?: InitInput; map?: Uint8Array } = {},
): Promise<LiveSkinPreview> {
    try {
        const [mod, bytes] = await Promise.all([
            module(options.wasm),
            options.map ? Promise.resolve(options.map) : mapBytes(options.fetchImpl ?? globalThis.fetch),
        ]);
        const preview: WasmPreview = new mod.SkinPreview(bytes, schemaJson, skinJson);
        return {
            width: preview.width,
            height: preview.height,
            setSkin: (next) => preview.set_skin(next),
            panBy: (dx, dy) => preview.pan_by(dx, dy),
            zoomAt: (factor, x, y) => preview.zoom_at(factor, x, y),
            resetCamera: () => preview.reset_camera(),
            stats: () => ({
                metersPerPixel: preview.meters_per_pixel,
                lodIndex: preview.lod_index,
                lodCount: preview.lod_count,
                featuresDrawn: preview.features_drawn,
                featuresDropped: preview.features_dropped,
                pointsDrawn: preview.points_drawn,
                spanUtilization: preview.span_utilization,
                pointUtilization: preview.point_utilization,
                ringUtilization: preview.ring_utilization,
            }),
            frame: () => preview.frame(),
            free: () => preview.free(),
        };
    } catch (cause) {
        throw new Error(`The live Teningen preview could not be opened (${describe(cause)}).`);
    }
}

/**
 * Render saved skins with one resident fixture/renderer, copying only each final
 * RGBA frame. Callers keep no wasm map/cache per card and persist no stale PNG.
 */
export async function renderSkinPreviewFrames(
    schemaJson: string,
    skins: readonly SkinEntry[],
    options: ThumbnailOptions = {},
): Promise<Record<string, SkinPreviewFrame>> {
    if (skins.length === 0) return {};
    const open = options.open ?? openLiveSkinPreview;
    const preview = await open(schemaJson, JSON.stringify(skins[0]));
    try {
        const frames: Record<string, SkinPreviewFrame> = {};
        for (const [index, skin] of skins.entries()) {
            if (options.signal?.aborted) break;
            preview.setSkin(JSON.stringify(skin));
            // wasm exposes a transient memory view. Each card needs an owned
            // snapshot before the next restamp overwrites that same frame.
            const pixels = new Uint8ClampedArray(preview.frame());
            frames[skin.id] = { width: preview.width, height: preview.height, pixels };
            if (index + 1 < skins.length) {
                const yieldToBrowser = options.yieldToBrowser ?? nextAnimationFrame;
                await yieldToBrowser();
            }
        }
        return frames;
    } finally {
        preview.free();
    }
}

function nextAnimationFrame(): Promise<void> {
    if (typeof globalThis.requestAnimationFrame !== "function") return Promise.resolve();
    return new Promise((resolve) => globalThis.requestAnimationFrame(() => resolve()));
}
