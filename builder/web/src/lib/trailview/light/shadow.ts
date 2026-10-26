import type { Gpu } from '../gpu/device';
import { wgsl } from '../gpu/wgsl';

/** Texels a side of each cascade's map (SHADOW_SIDE of shadow.wgsl), and its mip levels. */
const SIDE = 2048, LEVELS = Math.log2(SIDE) + 1;

/**
 * The cascades, near first. Each map holds the receivers within `half` metres of its centre across the
 * sun and within `hold` metres of its centre's height; along the sun it widens by `hold` × the cot of
 * the sun's altitude, the reach of their shadows on the level. `above`: the eye's height above the
 * ground where the cascade has faded out. `move`: the metres that the ground under the eye moves before
 * the map draws again; a quarter of `hold` up or down draws it too. `frames`: the least frames between
 * two draws for new casters while the eye moves; a still view draws them at once. `least`: the least height in metres of an instance that casts into it;
 * `proxies`: whether its trees cast as proxies (`proxy` of instances.ts).
 *
 * - near: 0.2 m texels across the sun, as the prototype's. `hold` 50 m keeps its texels along the sun
 *   under 0.5 m down to a sun of 10°. Chosen for cost: above 100 m the eye flies, and the near map
 *   would draw again every few frames; `move` keeps the eye 150 m from the map's side.
 * - far: 8 times the near reach, 1.6 m texels. `hold` 600 m: within 1638 m of a point of the kit's box
 *   below 1900 m, the ground lies within 600 m above or below it for 75 % of the points (box-terrain
 *   z10, measured). `move`, `frames`: the same share of the map, and a fourth of the near's draws.
 *   `least` 3 m: a smaller caster covers less than two of its texels. Its trees cast as proxies: with
 *   their meshes a draw rasterized for 4.5 to 6.4 ms (M1 Pro, kit walk and fly flights, measured).
 */
const CASCADES = [
    { half: 204.8, hold: 50, above: 100, move: 50, frames: 30, least: 0, proxies: false },
    { half: 1638.4, hold: 600, above: 1638.4, move: 400, frames: 120, least: 3, proxies: true },
];
/** Metres about the eye whose casters reach the near map at most, behind the eye too. */
export const NEAR_CASTERS_M = Math.SQRT2 * CASCADES[0].half;
/**
 * The metres along the sun's ray that a cast shadow reaches at most: there the sun's width (0.53°)
 * spreads its edge over 9.3 m, and a crown 10 m wide casts no core shadow. A map's casters stand up
 * to this far above its receivers.
 */
const REACH_M = 1000;
/** The z of the sun's direction below which object shadows fade out, at twice it fully there: under 3° a near texel stretches past 1 m along the sun. */
const LOW_SUN = 0.05;
/**
 * The sun moves this far (1 − cos) before the maps draw again: 0.026°, which the sun passes in 6 s, so
 * a step of the time slider by a minute draws them, and a move of the eye, which turns the sun's
 * direction in the frame by less, does not. Chosen.
 */
const SUN_TURN = 1e-7;
/** Bytes of `Shadows` in shadow.wgsl. */
const SHADOWS_BYTES = 208;

export const SHADOW_FORMAT: GPUTextureFormat = 'depth16unorm';
/**
 * The depth of a pipeline that casts into a map. The slope bias keeps a lit face out of its own
 * shadow; the faces of a caster draw from both sides.
 */
export const CASTER_DEPTH: GPUDepthStencilState = { format: SHADOW_FORMAT, depthWriteEnabled: true, depthCompare: 'less', depthBias: 4, depthBiasSlopeScale: 2 };
export const CASTER_PRIMITIVE: GPUPrimitiveState = { topology: 'triangle-list', cullMode: 'none' };

/** A cascade's state: the ground and the sun that its map was drawn for, whether new casters wait, and the frames since its draw. */
interface State { centre?: number[]; sun?: number[]; waiting: boolean; frames: number; active: boolean }

/** What changed for the casters this frame: instances or houses anywhere, and the nearest new terrain or structures in metres. */
export interface Casting { casters: boolean; reshaped: number }

interface Maps {
    texture: GPUTexture; frame: GPUBuffer; uniform: GPUBuffer; compare: GPUSampler;
    /** The pipeline of the mip levels, and per cascade and level above the first the group that reads the level below. */
    mips?: { pipeline: GPURenderPipeline; groups: GPUBindGroup[][] };
}

/**
 * The object shadows: per cascade one depth map of the casters seen from the sun, which every surface
 * reads (shadow.wgsl). Its projection shears along the sun onto the level of its centre and turns its
 * x axis toward the sun, so a texel across the sun has the same size at every altitude, and the depth
 * is the height. The scene holds still: a map draws again only when the eye leaves its centre, the sun
 * turns or new casters arrive, and one map at most draws in a frame.
 */
export class Shadow {
    private resources?: Maps;
    private readonly states: State[] = CASCADES.map(() => ({ waiting: false, frames: 0, active: false }));
    /** Per cascade the share that the receivers take from it by the eye's height, 0 without a map. */
    private readonly weights = CASCADES.map(() => 0);

    /** Takes the maps on a device at once, and resolves when their mip pipeline is compiled: no map draws before. */
    async attach(gpu: Gpu): Promise<void> {
        for (const state of this.states) state.centre = undefined;
        const { device } = gpu;
        const texture = gpu.texture('shadow', {
            size: [SIDE, SIDE, CASCADES.length], format: SHADOW_FORMAT, mipLevelCount: LEVELS, usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING,
        }, Math.round(SIDE * SIDE * 2 * CASCADES.length * 4 / 3));
        const r: Maps = {
            texture,
            // The casters' frame: Frame of terrain.wgsl with the sun's view projection.
            frame: gpu.buffer('shadow', 96, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            uniform: gpu.buffer('shadow', SHADOWS_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            compare: device.createSampler({ compare: 'less-equal', magFilter: 'linear', minFilter: 'linear' }),
        };
        this.resources = r;
        const pipeline = await gpu.once(mipPipeline);
        r.mips = {
            pipeline,
            groups: CASCADES.map((_, c) => Array.from({ length: LEVELS - 1 }, (_, k) => device.createBindGroup({
                layout: pipeline.getBindGroupLayout(0), entries: [{ binding: 0, resource: this.level(texture, c, k) }],
            }))),
        };
    }

    /** The casters' frame uniform. */
    get frame(): GPUBuffer {
        return this.resources!.frame;
    }

    /** Whether the near map holds the casters of the last frame, or the sun casts no near shadow. */
    get nearDrawn(): boolean {
        const near = this.states[0];
        return !near.active || (!!near.centre && !near.waiting);
    }

    /** Whether a map waits for a draw: the frames must go on. */
    get pending(): boolean {
        return this.states.some(state => state.active && (state.waiting || !state.centre));
    }

    /** Bindings 8 to 10 of the render group that the surfaces share. */
    entries(): GPUBindGroupEntry[] {
        const r = this.resources!;
        return [
            { binding: 8, resource: r.texture.createView({ dimension: '2d-array' }) }, { binding: 9, resource: r.compare },
            { binding: 10, resource: { buffer: r.uniform } },
        ];
    }

    /** The run of the instances' sun view for a cascade (SunRun of instances.ts). */
    run(cascade: number): { cascade: number; least: number; proxies: boolean } {
        const { least, proxies } = CASCADES[cascade];
        return { cascade, least, proxies };
    }

    /**
     * The cascade whose map this frame draws, or -1: for the ground under the eye (east, north, height),
     * the eye's height above it, the direction to the sun, what changed and whether the eye moved. Of
     * the cascades that are due, the one that waited longest draws. A cascade that the eye's height fades
     * out draws none, nor any under a sun that casts no object shadows.
     */
    due(ground: number[], above: number, sun: number[], casting: Casting, moving: boolean): number {
        let best = -1;
        CASCADES.forEach((c, k) => {
            const state = this.states[k], fade = 1 - smoothstep(c.above / 2, c.above, above);
            state.frames++;
            state.active = fade > 0 && sun[2] > LOW_SUN;
            if (!state.active) {
                state.centre = undefined;
                state.waiting = false;
                this.weights[k] = 0;
                return;
            }
            this.weights[k] = state.centre ? fade : 0;
            state.waiting ||= casting.casters || casting.reshaped < Math.SQRT2 * c.half;
            const { centre: o, sun: s } = state;
            const due = !o || !s || Math.hypot(ground[0] - o[0], ground[1] - o[1]) > c.move || Math.abs(ground[2] - o[2]) > c.hold / 4
                || 1 - (sun[0] * s[0] + sun[1] * s[1] + sun[2] * s[2]) > SUN_TURN || (state.waiting && (!moving || state.frames >= c.frames));
            if (due && (best < 0 || state.frames > this.states[best].frames)) best = k;
        });
        if (best < 0) return -1;
        const state = this.states[best];
        Object.assign(state, { centre: ground.slice(0, 3), sun: sun.slice(0, 3), waiting: false, frames: 0 });
        this.weights[best] = 1 - smoothstep(CASCADES[best].above / 2, CASCADES[best].above, above);
        return best;
    }

    /**
     * A cascade's projection from render space about the eye (east, north, height in the frame), columns
     * first: x along the sun and y across it, from -1 to 1 over the map, each snapped to whole texels of
     * the ground so that a draw at another centre puts each caster on the same texels; z the height from
     * the top of its casters (0) to the bottom of its receivers (1). Without a map it puts every point
     * past the map's edge.
     */
    matrix(cascade: number, eye: number[]): Float32Array<ArrayBuffer> {
        const m = new Float32Array(16), { centre, sun } = this.states[cascade];
        if (!centre || !sun) return m.fill(0).fill(2, 12, 13).fill(1, 15);
        const p = this.projection(cascade, centre, sun);
        const [ex, ey, ez] = eye;
        m.set([p.a / p.along, -p.b / p.across, 0, 0, p.b / p.along, p.a / p.across, 0, 0, -p.k / p.along, 0, -1 / p.depth, 0]);
        m.set([(p.a * ex + p.b * ey - p.k * ez - p.u) / p.along, (-p.b * ex + p.a * ey - p.v) / p.across, (p.top - ez) / p.depth, 1], 12);
        return m;
    }

    /** Writes the receivers' uniform (`Shadows` of shadow.wgsl) for the eye and the sun of this frame. */
    write(gpu: Gpu, eye: number[], sun: number[]): void {
        const data = new Float32Array(SHADOWS_BYTES / 4), lift = Math.max(sun[2], LOW_SUN);
        CASCADES.forEach((_, k) => {
            const { centre, sun: drawnSun } = this.states[k], at = 24 * k;
            data.set(this.matrix(k, eye), at);
            if (!centre || !drawnSun) return;
            const p = this.projection(k, centre, drawnSun);
            data.set([2 * p.along / SIDE, 2 * p.across / SIDE, p.depth, (p.top - centre[2] - CASCADES[k].hold) / p.depth, this.weights[k]], at + 16);
        });
        data.set([smoothstep(LOW_SUN, 2 * LOW_SUN, sun[2]), lift], 48);
        gpu.device.queue.writeBuffer(this.resources!.uniform, 0, data);
    }

    /** A render pass that clears a cascade's map for its casters. */
    pass(encoder: GPUCommandEncoder, cascade: number, timestampWrites?: GPURenderPassTimestampWrites): GPURenderPassEncoder {
        return encoder.beginRenderPass({
            label: 'shadow', colorAttachments: [], timestampWrites,
            depthStencilAttachment: { view: this.level(this.resources!.texture, cascade, 0), depthClearValue: 1, depthLoadOp: 'clear', depthStoreOp: 'store' },
        });
    }

    /** Encodes a cascade's mip levels after its casters drew; the timestamps span all of them. */
    encodeMips(encoder: GPUCommandEncoder, cascade: number, timing?: GPURenderPassTimestampWrites): void {
        const { pipeline, groups } = this.resources!.mips!, last = groups[cascade].length - 1;
        groups[cascade].forEach((group, k) => {
            const timestampWrites = timing && (k === 0 || k === last)
                ? { querySet: timing.querySet, beginningOfPassWriteIndex: k === 0 ? timing.beginningOfPassWriteIndex : undefined, endOfPassWriteIndex: k === last ? timing.endOfPassWriteIndex : undefined } : undefined;
            const pass = encoder.beginRenderPass({
                label: 'shadow mips', colorAttachments: [], timestampWrites,
                depthStencilAttachment: { view: this.level(this.resources!.texture, cascade, k + 1), depthLoadOp: 'clear', depthClearValue: 1, depthStoreOp: 'store' },
            });
            pass.setPipeline(pipeline);
            pass.setBindGroup(0, group);
            pass.draw(3);
            pass.end();
        });
    }

    /** The view of one mip level of a cascade's map. */
    private level(texture: GPUTexture, cascade: number, level: number): GPUTextureView {
        return texture.createView({ dimension: '2d', baseArrayLayer: cascade, arrayLayerCount: 1, baseMipLevel: level, mipLevelCount: 1 });
    }

    /**
     * A cascade's map about a centre for a sun: the sun's bearing (a, b), the shear k, the half sides
     * along and across the sun, the snapped offsets u and v of the centre, and the top and the span of
     * its heights.
     */
    private projection(cascade: number, [cx, cy, cz]: number[], [sx, sy, sz]: number[]) {
        const { half, hold } = CASCADES[cascade], level = Math.hypot(sx, sy), lift = Math.max(sz, LOW_SUN);
        const [a, b] = level > 1e-6 ? [sx / level, sy / level] : [1, 0], k = level / lift;
        const along = half + k * hold, top = cz + hold + REACH_M * lift;
        const snap = (value: number, side: number) => Math.round(value / (2 * side / SIDE)) * (2 * side / SIDE);
        return { a, b, k, along, across: half, u: snap(a * cx + b * cy - k * cz, along), v: snap(-b * cx + a * cy, half), top, depth: top - (cz - hold) };
    }
}

/** The pipeline that draws a mip level of a map from the level below. */
export function mipPipeline({ device }: Gpu): Promise<GPURenderPipeline> {
    const module = device.createShaderModule({ label: 'shadow mips', code: wgsl('light/shadow-mips.wgsl') });
    const layout = device.createBindGroupLayout({ entries: [{ binding: 0, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'depth' } }] });
    return device.createRenderPipelineAsync({
        label: 'shadow mips', layout: device.createPipelineLayout({ bindGroupLayouts: [layout] }), vertex: { module, entryPoint: 'mip_vertex' },
        fragment: { module, entryPoint: 'mip_fragment', targets: [] }, depthStencil: { format: SHADOW_FORMAT, depthWriteEnabled: true, depthCompare: 'always' },
    });
}

function smoothstep(edge0: number, edge1: number, x: number): number {
    const t = Math.min(1, Math.max(0, (x - edge0) / (edge1 - edge0)));
    return t * t * (3 - 2 * t);
}
