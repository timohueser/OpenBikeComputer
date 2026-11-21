import type { Gpu } from '../gpu/device';

export const DEPTH: GPUTextureFormat = 'depth32float';
/** The frame's colour on a device. */
export const sceneFormat = ({ device }: Gpu): GPUTextureFormat => (device.features.has('rg11b10ufloat-renderable') ? 'rg11b10ufloat' : 'rgba16float');
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
    /** The output of a wide frame, `margin` (horizontal, vertical) wider in tangent space, with whole pixels about the canvas; its history pair. */
    wideWidth = 0;
    wideHeight = 0;
    wideHistory: GPUTexture[] = [];
    wideHistoryDepth: GPUTexture[] = [];
    /** Whether the last frame wrote the wide history. */
    lastWide = false;

    /** `still`: the depth can be copied, for a still. A `margin` > 1 on an axis makes the wide targets. */
    constructor(private readonly gpu: Gpu, private readonly still = false, readonly margin: [number, number] = [1, 1]) {
        this.format = sceneFormat(gpu);
    }

    /** The render size of the frame at a scale, of the wide frame with `wide`. */
    viewport(scale: number, wide = false): [number, number] {
        const [w, h] = wide ? [this.wideWidth, this.wideHeight] : [this.width, this.height];
        return [Math.max(1, Math.round(w * scale)), Math.max(1, Math.round(h * scale))];
    }

    /** The history and its depth that a frame writes, wide or not. */
    written(wide: boolean): [GPUTexture, GPUTexture] {
        return wide ? [this.wideHistory[this.parity], this.wideHistoryDepth[this.parity]] : [this.history[this.parity], this.historyDepth[this.parity]];
    }

    /** The history and its depth that the last frame wrote. */
    last(): [GPUTexture, GPUTexture] {
        return this.lastWide ? [this.wideHistory[1 - this.parity], this.wideHistoryDepth[1 - this.parity]] : [this.history[1 - this.parity], this.historyDepth[1 - this.parity]];
    }

    /** Makes the targets for an output size and a largest render scale; true when they are new. */
    fit(width: number, height: number, top: number): boolean {
        if (width === this.width && height === this.height && top === this.top) return false;
        const { gpu } = this;
        for (const texture of [this.scene, this.depth, ...this.history, ...this.historyDepth, ...this.wideHistory, ...this.wideHistoryDepth]) if (texture) gpu.destroy('targets', texture);
        const pad = (n: number, margin: number) => n + 2 * Math.round(n * (margin - 1) / 2);
        Object.assign(this, { width, height, top, wideWidth: pad(width, this.margin[0]), wideHeight: pad(height, this.margin[1]), lastWide: false });
        const usage = GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING;
        const make = (format: GPUTextureFormat, bytes: number, [w, h]: number[], extra = 0) => gpu.texture('targets', { size: [w, h], format, usage: usage | extra }, bytes * w * h);
        const wides = this.margin.some(m => m > 1), render = this.viewport(top, wides), output = [width, height], wide = [this.wideWidth, this.wideHeight];
        this.scene = make(this.format, this.format === 'rg11b10ufloat' ? 4 : 8, render);
        this.depth = make(DEPTH, 4, render, this.still ? GPUTextureUsage.COPY_SRC : 0);
        this.history = [make(HISTORY, 4, output), make(HISTORY, 4, output)];
        this.historyDepth = [make(HISTORY_DEPTH, 2, output), make(HISTORY_DEPTH, 2, output)];
        if (wides) {
            this.wideHistory = [make(HISTORY, 4, wide), make(HISTORY, 4, wide)];
            this.wideHistoryDepth = [make(HISTORY_DEPTH, 2, wide), make(HISTORY_DEPTH, 2, wide)];
        }
        this.version++;
        return true;
    }

    /** After a frame: its history is the last one of the next frame. */
    swap(wide = false): void {
        this.parity ^= 1;
        this.lastWide = wide;
    }
}
