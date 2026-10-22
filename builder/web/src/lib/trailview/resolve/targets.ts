import type { Gpu } from '../gpu/device';

export const DEPTH: GPUTextureFormat = 'depth32float';
/** The history holds the accumulation in linear light: `rg11b10ufloat` would band a dusk sky. */
export const HISTORY: GPUTextureFormat = 'rgba16float';
/** The lowest render scale that the address takes. */
const MIN_SCALE = 0.25;

/** The render scale of an address: `scale` from MIN_SCALE to 1, else 1. */
export function scaleOf(search: string): number {
    const value = Number(new URLSearchParams(search).get('scale') ?? 1);
    return Number.isFinite(value) && value >= MIN_SCALE && value <= 1 ? value : 1;
}

/**
 * The render targets at the output size: the linear HDR frame, its depth, and the history pair, which
 * also keeps the depth of the last frame. A frame draws into the top left of the frame and the depth,
 * at the render scale, so a new scale makes no texture.
 */
export class Targets {
    readonly format: GPUTextureFormat;
    width = 0;
    height = 0;
    /** Bumps when the textures are made again. */
    version = 0;
    scene?: GPUTexture;
    depth?: GPUTexture;
    /** A frame writes the history of `parity` and reads the other one, the last frame's. */
    history: GPUTexture[] = [];
    parity = 0;

    constructor(private readonly gpu: Gpu, readonly scale: number) {
        this.format = gpu.device.features.has('rg11b10ufloat-renderable') ? 'rg11b10ufloat' : 'rgba16float';
    }

    /** The render size of the frame at a scale. */
    viewport(scale: number): [number, number] {
        return [Math.max(1, Math.round(this.width * scale)), Math.max(1, Math.round(this.height * scale))];
    }

    /** Makes the targets at an output size; true when they are new. */
    fit(width: number, height: number): boolean {
        if (width === this.width && height === this.height) return false;
        const { gpu } = this;
        for (const texture of [this.scene, this.depth, ...this.history]) if (texture) gpu.destroy('targets', texture);
        const usage = GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING, texels = width * height;
        const make = (format: GPUTextureFormat, bytes: number) => gpu.texture('targets', { size: [width, height], format, usage }, bytes * texels);
        this.scene = make(this.format, this.format === 'rg11b10ufloat' ? 4 : 8);
        this.depth = make(DEPTH, 4);
        this.history = [make(HISTORY, 8), make(HISTORY, 8)];
        this.width = width;
        this.height = height;
        this.version++;
        return true;
    }

    /** After a frame: its history is the last one of the next frame. */
    swap(): void {
        this.parity ^= 1;
    }
}
