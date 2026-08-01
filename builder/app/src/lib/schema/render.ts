import type { InitInput } from "../skin/pkg/obc_skin_preview.js";

type Bridge = typeof import("../skin/pkg/obc_skin_preview.js");
type WasmSchemaPreview = InstanceType<Bridge["SchemaPreview"]>;

export interface SchemaRenderStats {
    metersPerPixel: number;
    lodIndex: number;
    lodCount: number;
    chunksVisited: number;
    featuresTried: number;
    featuresDrawn: number;
    featuresDropped: number;
    pointsTried: number;
    pointsDrawn: number;
    spansUsed: number;
    ringsUsed: number;
    featureDecodeCapacityDrops: number;
    malformedFeatures: number;
    mapErrors: number;
}

export interface SchemaRenderLimits {
    maxFeaturePoints: number;
    maxFeatureRings: number;
    maxSpans: number;
    maxFramePoints: number;
    maxFrameRings: number;
}

export interface SchemaRenderer {
    readonly width: number;
    readonly height: number;
    setMetersPerPixel(value: number): void;
    readonly limits: SchemaRenderLimits;
    frame(): Uint8ClampedArray;
    stats(): SchemaRenderStats;
    free(): void;
}

let loading: Promise<Bridge> | null = null;

async function module(source?: InitInput): Promise<Bridge> {
    if (!loading) {
        const pending = (async () => {
            const mod = await import("../skin/pkg/obc_skin_preview.js");
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

export async function openSchemaRenderer(bytes: Uint8Array, wasm?: InitInput): Promise<SchemaRenderer> {
    const mod = await module(wasm);
    const preview: WasmSchemaPreview = new mod.SchemaPreview(bytes);
    return {
        width: preview.width,
        height: preview.height,
        limits: {
            maxFeaturePoints: preview.max_feature_points,
            maxFeatureRings: preview.max_feature_rings,
            maxSpans: preview.max_spans,
            maxFramePoints: preview.max_frame_points,
            maxFrameRings: preview.max_frame_rings,
        },
        setMetersPerPixel: (value) => preview.set_meters_per_pixel(value),
        frame: () => preview.frame(),
        stats: () => ({
            metersPerPixel: preview.meters_per_pixel,
            lodIndex: preview.lod_index,
            lodCount: preview.lod_count,
            chunksVisited: preview.chunks_visited,
            featuresTried: preview.features_tried,
            featuresDrawn: preview.features_drawn,
            featuresDropped: preview.features_dropped,
            pointsTried: preview.points_tried,
            pointsDrawn: preview.points_drawn,
            spansUsed: preview.spans_used,
            ringsUsed: preview.rings_used,
            featureDecodeCapacityDrops: preview.feature_decode_capacity_drops,
            malformedFeatures: preview.malformed_features,
            mapErrors: preview.map_errors,
        }),
        free: () => preview.free(),
    };
}
