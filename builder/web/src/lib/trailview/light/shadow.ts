import type { Gpu } from '../gpu/device';

/** Texels a side of the map, and its half side in metres on the ground: 0.2 m per texel (SIDE of shadow.wgsl). */
const SIDE = 2048, HALF_M = 204.8, TEXEL_M = (2 * HALF_M) / SIDE;
/** Metres above and below the map's centre that its depth holds. */
const DEPTH_M = 1000;
/** The map draws again when the eye is this far from its centre on the ground. */
const MOVE_M = 50;
/** While the camera moves, new content draws the map again after this many frames at most. */
const CONTENT_FRAMES = 30;
/** The least z of the sun's direction that the map shears by: a lower sun casts no longer. */
const LOW_SUN = 0.05;
/** The sun moves this far (1 − cos) before the map draws again: a move of the eye does not. */
const SUN_TURN = 1e-7;

export const SHADOW_FORMAT: GPUTextureFormat = 'depth16unorm';
/**
 * The depth of a pipeline that casts into the map. The slope bias keeps a lit face out of its own
 * shadow; the faces of a caster draw from both sides.
 */
export const CASTER_DEPTH: GPUDepthStencilState = { format: SHADOW_FORMAT, depthWriteEnabled: true, depthCompare: 'less', depthBias: 4, depthBiasSlopeScale: 2 };
export const CASTER_PRIMITIVE: GPUPrimitiveState = { topology: 'triangle-list', cullMode: 'none' };

/**
 * The object shadows: one depth map of the casters seen from the sun, ±HALF_M about a centre on the
 * ground, which every surface reads (shadow.wgsl). Its projection shears along the sun onto the level
 * ground of the centre, so a texel is TEXEL_M of the ground at every altitude of the sun, and its depth
 * is the height. The scene holds still: the map draws again only when the eye leaves its centre, the
 * sun turns or new content arrives.
 */
export class Shadow {
    private resources?: { texture: GPUTexture; frame: GPUBuffer; sampler: GPUSampler };
    /** The centre of the drawn map, east, north and height in the frame, and the sun that it was drawn for. */
    private drawn?: { centre: number[]; sun: number[] };
    /** Whether the eye is too high for the map: then no surface reads it. */
    private high = false;
    /** Whether new content waits for a draw, and the frames since the last draw. */
    private waiting = false;
    private frames = 0;

    attach(gpu: Gpu): void {
        this.drawn = undefined;
        this.resources = {
            texture: gpu.texture('shadow', { size: [SIDE, SIDE], format: SHADOW_FORMAT, usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING }, SIDE * SIDE * 2),
            // The casters' frame: Frame of terrain.wgsl with the sun's view projection.
            frame: gpu.buffer('shadow', 96, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            sampler: gpu.device.createSampler({ compare: 'less-equal', magFilter: 'linear', minFilter: 'linear' }),
        };
    }

    /** The casters' frame uniform. */
    get frame(): GPUBuffer {
        return this.resources!.frame;
    }

    /** Bindings 8 and 9 of the render group that the surfaces share. */
    entries(): GPUBindGroupEntry[] {
        const r = this.resources!;
        return [{ binding: 8, resource: r.texture.createView() }, { binding: 9, resource: r.sampler }];
    }

    /**
     * Whether this frame draws the map, for the ground under the eye (east, north, height) and the eye's
     * height above it, and the direction to the sun. New content (`changed`) draws it at once in a still
     * view, and at most every CONTENT_FRAMES frames while the camera moves. An eye higher above the
     * ground than HALF_M sees the map from afar and mostly under it: it draws none, and reads none.
     */
    due(ground: number[], above: number, sun: number[], changed: boolean, moving: boolean): boolean {
        this.waiting ||= changed;
        this.frames++;
        this.high = above > HALF_M;
        if (this.high) return false;
        const d = this.drawn, left = !d || Math.hypot(ground[0] - d.centre[0], ground[1] - d.centre[1]) > MOVE_M;
        const due = left || 1 - (sun[0] * d.sun[0] + sun[1] * d.sun[1] + sun[2] * d.sun[2]) > SUN_TURN
            || (this.waiting && (!moving || this.frames >= CONTENT_FRAMES));
        if (!due) return false;
        // On the texels of the ground: a draw at the same centre puts each caster on the same texels.
        const centre = left ? [Math.round(ground[0] / TEXEL_M) * TEXEL_M, Math.round(ground[1] / TEXEL_M) * TEXEL_M, Math.round(ground[2])] : d.centre;
        this.drawn = { centre, sun: sun.slice(0, 3) };
        this.waiting = false;
        this.frames = 0;
        return true;
    }

    /**
     * The map's projection from render space about the eye (east, north, height in the frame), columns
     * first: x and y the ground point that the sun's ray through a point meets at the centre's height,
     * z the height from DEPTH_M above the centre (0) to DEPTH_M below (1). Without a map it puts every
     * point past the map's edge.
     */
    matrix(eye: number[]): Float32Array<ArrayBuffer> {
        const m = new Float32Array(16), d = this.drawn;
        if (!d || this.high) return m.fill(0).fill(2, 12, 13).fill(1, 15);
        const [cx, cy, cz] = d.centre, kx = d.sun[0] / Math.max(d.sun[2], LOW_SUN), ky = d.sun[1] / Math.max(d.sun[2], LOW_SUN);
        const dz = eye[2] - cz;
        m.set([1 / HALF_M, 0, 0, 0, 0, 1 / HALF_M, 0, 0, -kx / HALF_M, -ky / HALF_M, -1 / (2 * DEPTH_M), 0]);
        m.set([(eye[0] - cx - kx * dz) / HALF_M, (eye[1] - cy - ky * dz) / HALF_M, (DEPTH_M - dz) / (2 * DEPTH_M), 1], 12);
        return m;
    }

    /** A render pass that clears the map for the casters. */
    pass(encoder: GPUCommandEncoder, timestampWrites?: GPURenderPassTimestampWrites): GPURenderPassEncoder {
        return encoder.beginRenderPass({
            label: 'shadow', colorAttachments: [], timestampWrites,
            depthStencilAttachment: { view: this.resources!.texture.createView(), depthClearValue: 1, depthLoadOp: 'clear', depthStoreOp: 'store' },
        });
    }
}
