import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { DEPTH, sceneFormat } from '../resolve/targets';
import type { Mat3 } from './celestial';
import type { NightSky } from './night';

/** `Night` of sky.wgsl. */
const NIGHT_BYTES = 112;
/** Bytes of a star in the vertex buffer: its J2000 direction and V magnitude as floats, and its colour in bytes. */
const STAR_BYTES = 20;
// The files of tools/trailview/sky.py.
const MILKY_WAY = new URL('./milkyway.webp', import.meta.url), STARS = new URL('./stars.bin', import.meta.url), MOON = new URL('./moon.webp', import.meta.url);
/** The `attribution` of the sources `nasa-svs-milky-way`, `nasa-svs-moon` and `yale-bright-stars` in data/sources.toml, shortened. */
export const SKY_CREDITS = 'Milky Way and moon: NASA/GSFC SVS, ESA/Gaia/DPAC; stars: Yale Bright Star Catalogue';
/** Star colours by B−V: blue-white, white, orange; an eye sees them pale. */
const TINTS: [number, number[]][] = [[-0.3, [0.68, 0.78, 1]], [0.6, [1, 0.97, 0.92]], [1.6, [1, 0.74, 0.5]]];

/** What the sky shows this frame. */
export interface SkyState {
    /** J2000 equatorial directions to render space, rows first. */
    stars: Mat3;
    /** Toward the moon in render space. */
    moon: number[];
    /** The colour that the air gives the moon's disk. */
    moonAir: number[];
    /** Whether the moon is the light in the sun's place. */
    moonLight: boolean;
    night: NightSky;
    /** The moon disk's share. */
    moonDisk: number;
    /** Clip space per render pixel. */
    pixel: [number, number];
}

interface Stars { buffer: GPUBuffer; magnitudes: Float32Array; pipeline: GPURenderPipeline }

/**
 * The sky pass: one triangle at depth 0 behind the terrain, then the stars. Both read the look of
 * shade.wgsl in group 2 and `Night` in group 0. Until the stars, the Milky Way and the moon's face
 * have loaded (`attachNight`), the sky draws without them and the moon plain grey.
 */
export class Sky {
    /** Whether the stars, the Milky Way or the moon's face arrived since the last `write`: the frame changes. */
    newContent = false;
    /** Whether `attachNight` still loads. */
    private loading = false;
    /** The device of the last `attach`: a load for an older one comes to nothing. */
    private gpu?: Gpu;
    private pipeline?: GPURenderPipeline;
    private empty?: GPUBindGroup;
    private uniform?: GPUBuffer;
    private group?: GPUBindGroup;
    private stars?: Stars;
    /** The stars brighter than this frame's limit: the buffer holds them brightest first. */
    private count = 0;

    async attach(gpu: Gpu): Promise<void> {
        const pipeline = await gpu.once(skyPipeline);
        this.empty = gpu.device.createBindGroup({ layout: gpu.device.createBindGroupLayout({ entries: [] }), entries: [] });
        this.uniform = gpu.buffer('uniforms', NIGHT_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        this.group = this.night(gpu, pixel(gpu, [0, 0, 0, 255]), pixel(gpu, [170, 170, 170, 255]));
        this.stars = undefined;
        this.pipeline = pipeline;
        this.gpu = gpu;
    }

    /** Whether nothing loads. */
    get idle(): boolean {
        return !this.loading;
    }

    /**
     * Loads the Milky Way, the stars and the moon's face, and compiles the stars' pipeline. It never
     * fails: without them the sky draws on as before. Nothing is made on a device that `attach` left.
     */
    async attachNight(gpu: Gpu): Promise<void> {
        this.loading = true;
        try {
            const [milkyWay, face, table, pipeline] = await Promise.all([bitmap(MILKY_WAY), bitmap(MOON), fetched(STARS).then(response => response.arrayBuffer()), gpu.once(starPipeline)]);
            if (!table.byteLength || table.byteLength % 6) throw new Error(`${STARS} is not a table of stars`);
            if (this.gpu !== gpu) return;
            const { data, magnitudes } = starVertices(table), buffer = gpu.buffer('sky', data.byteLength, GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST);
            gpu.device.queue.writeBuffer(buffer, 0, data);
            this.group = this.night(gpu, texture(gpu, milkyWay), texture(gpu, face));
            this.stars = { buffer, magnitudes, pipeline };
            this.newContent = true;
        } catch (error) {
            console.warn('TrailView draws the sky without its stars, Milky Way and moon face:', error);
        } finally {
            if (this.gpu === gpu) this.loading = false;
        }
    }

    /** Writes `Night` of sky.wgsl for this frame. */
    write(gpu: Gpu, state: SkyState): void {
        this.newContent = false;
        const data = new Float32Array(NIGHT_BYTES / 4), { stars: m, night } = state;
        // mat3x3f: three columns of four floats.
        data.set([m[0], m[3], m[6], 0, m[1], m[4], m[7], 0, m[2], m[5], m[8], 0], 0);
        data.set([...state.moon, state.moonLight ? 1 : 0], 12);
        data.set([night.limit, night.stars, night.milkyWay, state.moonDisk], 16);
        data.set([...state.pixel, 0, 0], 20);
        data.set([...state.moonAir, 0], 24);
        gpu.device.queue.writeBuffer(this.uniform!, 0, data);
        const magnitudes = this.stars?.magnitudes;
        this.count = magnitudes && night.stars > 0 ? below(magnitudes, night.limit) : 0;
    }

    draw(pass: GPURenderPassEncoder, look: GPUBindGroup): void {
        if (!this.pipeline || !this.empty || !this.group) return;
        pass.setPipeline(this.pipeline);
        pass.setBindGroup(0, this.group);
        pass.setBindGroup(1, this.empty);
        pass.setBindGroup(2, look);
        pass.draw(3);
        if (!this.stars || !this.count) return;
        pass.setPipeline(this.stars.pipeline);
        pass.setVertexBuffer(0, this.stars.buffer);
        pass.draw(4, this.count);
    }

    private night(gpu: Gpu, milkyWay: GPUTexture, face: GPUTexture): GPUBindGroup {
        return gpu.device.createBindGroup({
            layout: gpu.once(nightLayout), entries: [
                { binding: 0, resource: { buffer: this.uniform! } }, { binding: 1, resource: milkyWay.createView() },
                { binding: 2, resource: gpu.device.createSampler({ magFilter: 'linear', minFilter: 'linear', addressModeU: 'repeat' }) },
                { binding: 3, resource: face.createView() },
            ],
        });
    }
}

/** The answer to a request that succeeded. */
async function fetched(url: URL): Promise<Response> {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: ${response.status}`);
    return response;
}

/** An image file, decoded without a colour conversion. */
async function bitmap(url: URL): Promise<ImageBitmap> {
    return createImageBitmap(await (await fetched(url)).blob(), { colorSpaceConversion: 'none' });
}

/** An sRGB image as a texture. */
function texture(gpu: Gpu, image: ImageBitmap): GPUTexture {
    const made = gpu.texture('sky', {
        size: [image.width, image.height], format: 'rgba8unorm-srgb', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
    }, 4 * image.width * image.height);
    gpu.device.queue.copyExternalImageToTexture({ source: image }, { texture: made }, [image.width, image.height]);
    return made;
}

/** A texture of one sRGB texel, which stands for an image until it loads. */
function pixel(gpu: Gpu, rgba: number[]): GPUTexture {
    const texture = gpu.texture('sky', { size: [1, 1], format: 'rgba8unorm-srgb', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST }, 4);
    gpu.device.queue.writeTexture({ texture }, new Uint8Array(rgba), {}, [1, 1]);
    return texture;
}

/** The stars of `stars.bin` (tools/trailview/sky.py) as vertices, brightest first, and their magnitudes. */
export function starVertices(table: ArrayBuffer): { data: ArrayBuffer; magnitudes: Float32Array } {
    const count = table.byteLength / 6, read = new DataView(table), data = new ArrayBuffer(count * STAR_BYTES), write = new DataView(data);
    const magnitudes = new Float32Array(count);
    for (let i = 0; i < count; i++) {
        const ra = read.getUint16(6 * i, true) / 65536 * 2 * Math.PI, dec = read.getInt16(6 * i + 2, true) / 32767 * Math.PI / 2;
        const v = read.getUint8(6 * i + 4) / 20 - 2, bv = read.getUint8(6 * i + 5) / 80 - 0.5;
        magnitudes[i] = v;
        [Math.cos(dec) * Math.cos(ra), Math.cos(dec) * Math.sin(ra), Math.sin(dec), v].forEach((value, k) => write.setFloat32(STAR_BYTES * i + 4 * k, value, true));
        tint(bv).forEach((value, k) => write.setUint8(STAR_BYTES * i + 16 + k, Math.round(255 * value)));
    }
    return { data, magnitudes };
}

function tint(bv: number): number[] {
    const next = TINTS.findIndex(([at]) => at > bv), upper = next < 0 ? TINTS.length - 1 : Math.max(next, 1), [a, ca] = TINTS[upper - 1], [b, cb] = TINTS[upper];
    const t = Math.min(Math.max((bv - a) / (b - a), 0), 1);
    return ca.map((value, k) => value + (cb[k] - value) * t);
}

/** How many of the ascending `values` lie below `limit`. */
function below(values: Float32Array, limit: number): number {
    let lo = 0, hi = values.length;
    while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (values[mid] < limit) lo = mid + 1;
        else hi = mid;
    }
    return lo;
}

/** Group 0 of the sky and the stars: `Night`, the Milky Way, their sampler and the moon's face. */
function nightLayout({ device }: Gpu): GPUBindGroupLayout {
    const both = GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT;
    return device.createBindGroupLayout({
        entries: [
            { binding: 0, visibility: both, buffer: { type: 'uniform' } },
            { binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: {} }, { binding: 2, visibility: GPUShaderStage.FRAGMENT, sampler: {} },
            { binding: 3, visibility: GPUShaderStage.FRAGMENT, texture: {} },
        ],
    });
}

function skyLayout(gpu: Gpu): GPUPipelineLayout {
    return gpu.device.createPipelineLayout({ bindGroupLayouts: [gpu.once(nightLayout), gpu.device.createBindGroupLayout({ entries: [] }), gpu.once(layouts).ground] });
}

const skyModule = ({ device }: Gpu) => device.createShaderModule({ label: 'sky', code: wgsl('light/sky.wgsl') });

export function skyPipeline(gpu: Gpu): Promise<GPURenderPipeline> {
    const module = gpu.once(skyModule);
    return gpu.device.createRenderPipelineAsync({
        label: 'sky', layout: skyLayout(gpu),
        vertex: { module, entryPoint: 'sky_vertex' },
        fragment: { module, entryPoint: 'sky_fragment', targets: [{ format: sceneFormat(gpu) }] },
        depthStencil: { format: DEPTH, depthWriteEnabled: false, depthCompare: 'equal' },
    });
}

/** The stars: a quad per star, added to the sky where no terrain covers it. */
function starPipeline(gpu: Gpu): Promise<GPURenderPipeline> {
    const module = gpu.once(skyModule), add: GPUBlendComponent = { srcFactor: 'one', dstFactor: 'one', operation: 'add' };
    return gpu.device.createRenderPipelineAsync({
        label: 'stars', layout: skyLayout(gpu),
        vertex: {
            module, entryPoint: 'star_vertex',
            buffers: [{ arrayStride: STAR_BYTES, stepMode: 'instance', attributes: [{ shaderLocation: 0, offset: 0, format: 'float32x4' }, { shaderLocation: 1, offset: 16, format: 'unorm8x4' }] }],
        },
        fragment: { module, entryPoint: 'star_fragment', targets: [{ format: sceneFormat(gpu), blend: { color: add, alpha: add } }] },
        primitive: { topology: 'triangle-strip' },
        depthStencil: { format: DEPTH, depthWriteEnabled: false, depthCompare: 'equal' },
    });
}
