/**
 * Lazy browser bridge for the live skin preview.
 *
 * The OBCM is the bakery's canonical Teningen fixture, emitted by Vite as a
 * separate asset rather than copied into the JS or wasm module. Opening the
 * editor fetches the fixture and uses the shared builder renderer; ordinary
 * skin picking uses the small digest-pinned PNGs.
 */

import type { InitInput } from "../core/pkg/obc_builder_bridge.js";
import { initCore } from "../core/bridge";
import type { SkinEntry } from "../catalog/manifest";
import { skinStyleError } from "./validation";

const MAP_URL = new URL("../../../../../host/obc-bake/assets/teningen-preview.obcm", import.meta.url);

type Bridge = typeof import("../core/pkg/obc_builder_bridge.js");
type WasmPreview = InstanceType<Bridge["SkinPreview"]>;

export interface LiveSkinPreview {
    readonly width: number;
    readonly height: number;
    setStyles(lightSkinJson: string, darkSkinJson: string): void;
    setTheme(dark: boolean): void;
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

let mapLoading: Promise<Uint8Array> | null = null;

function describe(cause: unknown): string {
    return cause instanceof Error ? cause.message : String(cause);
}

function module(source?: InitInput): Promise<Bridge> {
    return initCore(source);
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
    lightSkinJson: string,
    darkSkinJson: string,
    options: { fetchImpl?: typeof fetch; wasm?: InitInput; map?: Uint8Array } = {},
): Promise<LiveSkinPreview> {
    try {
        const parsed = JSON.parse(schemaJson);
        const schema = parsed.schema ?? parsed;
        const admit = (json: string): void => {
            const error = skinStyleError(schema, JSON.parse(json).styles);
            if (error) throw new Error(error);
        };
        admit(lightSkinJson);
        admit(darkSkinJson);
        const [mod, bytes] = await Promise.all([
            module(options.wasm),
            options.map ? Promise.resolve(options.map) : mapBytes(options.fetchImpl ?? globalThis.fetch),
        ]);
        const preview: WasmPreview = new mod.SkinPreview(bytes, schemaJson, lightSkinJson, darkSkinJson);
        return {
            width: preview.width,
            height: preview.height,
            setStyles: (light, dark) => {
                admit(light);
                admit(dark);
                preview.set_styles(light, dark);
            },
            setTheme: (dark) => preview.set_theme(dark),
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
    const first = JSON.stringify(skins[0]);
    const preview = await open(schemaJson, first, first);
    try {
        const frames: Record<string, SkinPreviewFrame> = {};
        for (const [index, skin] of skins.entries()) {
            if (options.signal?.aborted) break;
            const json = JSON.stringify(skin);
            preview.setStyles(json, json);
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
