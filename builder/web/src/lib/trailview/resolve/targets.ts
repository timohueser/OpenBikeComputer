import type { Gpu } from '../gpu/device';

export const DEPTH: GPUTextureFormat = 'depth32float';
/** The history's colour: 10 bits per channel, encoded in resolve.wgsl so a dusk sky shows no bands. */
export const HISTORY: GPUTextureFormat = 'rgb10a2unorm';
/** The history's depth: the nearest depth about each pixel of the last frame. */
export const HISTORY_DEPTH: GPUTextureFormat = 'r16float';
/** The render scales that the address takes. The frame and the depth are made for MAX_SCALE. */
const MIN_SCALE = 0.25;
export const MAX_SCALE = 0.75;
const DEFAULT_SCALE = 0.7;

/** The render scale of an address: `scale` from MIN_SCALE to MAX_SCALE, else DEFAULT_SCALE. */
export function scaleOf(search: string): number {
    const value = Number(new URLSearchParams(search).get('scale') ?? DEFAULT_SCALE);
    return Number.isFinite(value) && value >= MIN_SCALE && value <= MAX_SCALE ? value : DEFAULT_SCALE;
}

/**
 * The render targets: the linear HDR frame and its depth for the largest render scale `top`, and the
 * history pair at the output size. A frame draws into the top left of the frame and the depth, so a
 * new scale up to `top` makes no texture.
 */
export class Targets {
    readonly format: GPUTextureFormat;
    width = 0;
    height = 0;
    top = 0;
    /** Bumps when the textures are made again. */
    version = 0;
    scene?: GPUTexture;
    depth?: GPUTexture;
    /** A frame writes the history of `parity` and reads the other one, the last frame's. */
    history: GPUTexture[] = [];
    historyDepth: GPUTexture[] = [];
    parity = 0;

    constructor(private readonly gpu: Gpu) {
        this.format = gpu.device.features.has('rg11b10ufloat-renderable') ? 'rg11b10ufloat' : 'rgba16float';
    }

    /** The render size of the frame at a scale. */
    viewport(scale: number): [number, number] {
        return [Math.max(1, Math.round(this.width * scale)), Math.max(1, Math.round(this.height * scale))];
    }

    /** Makes the targets for an output size and a largest render scale; true when they are new. */
    fit(width: number, height: number, top: number): boolean {
        if (width === this.width && height === this.height && top === this.top) return false;
        const { gpu } = this;
        for (const texture of [this.scene, this.depth, ...this.history, ...this.historyDepth]) if (texture) gpu.destroy('targets', texture);
        Object.assign(this, { width, height, top });
        const usage = GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING;
        const make = (format: GPUTextureFormat, bytes: number, [w, h]: number[]) => gpu.texture('targets', { size: [w, h], format, usage }, bytes * w * h);
        const render = this.viewport(top), output = [width, height];
        this.scene = make(this.format, this.format === 'rg11b10ufloat' ? 4 : 8, render);
        this.depth = make(DEPTH, 4, render);
        this.history = [make(HISTORY, 4, output), make(HISTORY, 4, output)];
        this.historyDepth = [make(HISTORY_DEPTH, 2, output), make(HISTORY_DEPTH, 2, output)];
        this.version++;
        return true;
    }

    /** After a frame: its history is the last one of the next frame. */
    swap(): void {
        this.parity ^= 1;
    }
}
