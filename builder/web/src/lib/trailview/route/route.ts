import type { Coordinate } from '../../planner/geo';
import { worldPixel } from '../../planner/layers/mercator';
import type { Frame } from '../frame/frame';
import { UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { metresPerUnit } from '../terrain/bins';

/** Words of a point, as `Point` of route.wgsl: x, y (int32 world units), its colour as sRGB bytes and the number of its route. */
export const POINT_WORDS = 4;
/** The line bends at most this far from the surface between two points. */
const STEP_M = 5;
/** The side of a badge image in image pixels, as BADGE_PIXELS of route.wgsl: twice its output pixels. */
export const BADGE_SIZE = 64;
/** Words of a pin, as `Pin` of route.wgsl. */
const PIN_WORDS = 4;
/** The share of its light that a pin shows where something hides it. */
const HIDDEN_PIN = 0.35;

/** A pin on the ground: a stem over a ring in its colour, with its badge on top, or without one a sphere in its colour. */
export interface RoutePin { coordinate: Coordinate; colour: string; badge?: GPUCopyExternalImageSource }

/** A route on the surface: its line in riding order and the colour of each edge. */
export interface RouteLine {
    line: readonly Coordinate[];
    colours: readonly string[];
    /** The height in metres at each `step` metres along the route and at its end, as `profile_m` of the route catalog. Only a route with a profile has pick points. */
    profile?: { step: number; heights: readonly (number | null)[] };
}

/**
 * The points of the routes in riding order, each with the number of its route, from 1: the vertices of a
 * line, and between them points at most STEP_M apart, so that the ribbon follows the surface. A point
 * takes the colour of the edge that ends at it, the first one that of the first edge. Between two routes
 * that draw stand two break points with the number 0: a copy of the last point of the one route and a
 * copy of the first point of the next. A route of fewer than two points draws none.
 */
export function routePoints(routes: readonly RouteLine[]): Uint32Array<ArrayBuffer> {
    const words: number[] = [];
    const push = (x: number, y: number, colour: number, route: number) => words.push(Math.round(x), Math.round(y), colour, route);
    routes.forEach(({ line, colours }, k) => {
        if (line.length < 2) return;
        const [x0, y0] = worldPixel(line[0], UNITS), first = packed(colours[0]);
        if (words.length) {
            push(words.at(-4)!, words.at(-3)!, words.at(-2)!, 0);
            push(x0, y0, first, 0);
        }
        push(x0, y0, first, k + 1);
        for (let i = 1; i < line.length; i++) {
            const [x, y] = worldPixel(line[i], UNITS), [lx, ly] = worldPixel(line[i - 1], UNITS), metres = Math.hypot(x - lx, y - ly) * metresPerUnit((y + ly) / 2);
            const steps = Math.ceil(metres / STEP_M), colour = packed(colours[i - 1]);
            for (let s = 1; s <= steps; s++) push(lx + (x - lx) * s / steps, ly + (y - ly) * s / steps, colour, k + 1);
        }
    });
    return new Uint32Array(new Int32Array(words).buffer);
}

/**
 * The pick points of a route: frame metres east and north and the height of each entry of its profile
 * that has one, at its place on the line. Entry i lies at the share i × step / L of the line, where L is
 * the length of the route, and the last entry at its end. The profile gives L only within a step,
 * (n − 2) × step ≤ L < (n − 1) × step for n entries: the length of the line, held in that range, stands
 * for it. Undefined for a route with no profile or a line with no length.
 */
export function pickPoints({ line, profile }: RouteLine, frame: Frame): Float64Array | undefined {
    if (!profile || line.length < 2 || profile.heights.length < 2) return undefined;
    const { step, heights } = profile, at = line.map(point => frame.local(...point)), along = [0];
    for (let i = 1; i < at.length; i++) along.push(along[i - 1] + Math.hypot(at[i][0] - at[i - 1][0], at[i][1] - at[i - 1][1]));
    const total = along.at(-1)!;
    if (!total) return undefined;
    const length = Math.min(Math.max(total, (heights.length - 2) * step), (heights.length - 1) * step), out: number[] = [];
    for (let e = 0, i = 1; e < heights.length; e++) {
        const height = heights[e];
        if (height === null) continue;
        const d = Math.min(e * step / length, 1) * total;
        while (i < at.length - 1 && along[i] < d) i++;
        const t = along[i] > along[i - 1] ? (d - along[i - 1]) / (along[i] - along[i - 1]) : 0;
        out.push(at[i - 1][0] + t * (at[i][0] - at[i - 1][0]), at[i - 1][1] + t * (at[i][1] - at[i - 1][1]), height);
    }
    return new Float64Array(out);
}

/** '#rrggbb' as the bytes of WGSL's unpack4x8unorm: red lowest. */
function packed(hex: string): number {
    return parseInt(hex.slice(5, 7) + hex.slice(3, 5) + hex.slice(1, 3), 16);
}

interface Resources {
    gpu: Gpu;
    line: GPURenderPipeline;
    /** The pins where they are in sight, and where something hides them. */
    pin: GPURenderPipeline[];
    layout: GPUBindGroupLayout;
    frame: GPUBuffer;
    /** The number of the bold route, 0 for none. */
    bold: GPUBuffer;
    empty: GPUBindGroup;
    sampler: GPUSampler;
    points?: GPUBuffer;
    pins?: GPUBuffer;
    badges?: GPUTexture;
    group?: GPUBindGroup;
}

/** The routes on the drawn surface and the pins on the ground, with route.wgsl. */
export class Route {
    /** Whether a route or a pin changed since the last draw. */
    newContent = false;
    /** The pick points of each route (`pickPoints`), by its number less one. */
    picks: readonly (Float64Array | undefined)[] = [];
    /** The number of the route that draws bold and on top, or null for none. */
    bold: number | null = null;
    private points = new Uint32Array(0);
    private pins: readonly RoutePin[] = [];
    /** The pin that follows the rider's pointer, after the others. */
    private marker?: RoutePin;
    private resources?: Resources;
    /** What is not on the GPU yet. */
    private fresh = { points: true, pins: true, badges: true, bold: true };

    /** The routes in place of the ones before, numbered from 1 in their order; `frame` places their pick points. */
    set(routes: readonly RouteLine[], frame: Frame): void {
        this.points = routePoints(routes);
        this.picks = routes.map(route => pickPoints(route, frame));
        this.fresh.points = this.newContent = true;
    }

    setBold(route: number | null): void {
        this.bold = route;
        this.fresh.bold = this.newContent = true;
    }

    setPins(pins: readonly RoutePin[]): void {
        this.pins = pins;
        this.fresh.pins = this.fresh.badges = this.newContent = true;
    }

    /** A pin without a badge, which moves without loading the badges again. */
    setMarker(marker: Omit<RoutePin, 'badge'> | undefined): void {
        this.marker = marker;
        this.fresh.pins = this.newContent = true;
    }

    /** `frame` is the frame uniform of the eye's view. */
    async attach(gpu: Gpu, frame: GPUBuffer): Promise<void> {
        this.resources = undefined;
        const { line, pin, layout, empty } = await gpu.once(routePipelines);
        this.resources = {
            gpu, line, pin, layout, frame, bold: gpu.buffer('route', 4, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            empty: gpu.device.createBindGroup({ layout: empty, entries: [] }), sampler: gpu.device.createSampler({ magFilter: 'linear', minFilter: 'linear' }),
        };
        this.fresh = { points: true, pins: true, badges: true, bold: true };
    }

    draw(pass: GPURenderPassEncoder, surface: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        this.newContent = false;
        if (!r) return;
        if (this.fresh.points || this.fresh.pins || this.fresh.badges || this.fresh.bold) this.write(r);
        if (!r.group) return;
        [r.group, surface, r.empty, features].forEach((group, k) => pass.setBindGroup(k, group));
        if (this.points.length) {
            pass.setPipeline(r.line);
            // The second instance draws the bold route alone, after the first has drawn the others.
            pass.draw(2 * this.points.length / POINT_WORDS, this.bold ? 2 : 1);
        }
        const pins = this.pins.length + (this.marker ? 1 : 0);
        if (pins) for (const pipeline of r.pin) {
            pass.setPipeline(pipeline);
            // A ring, a stem and a head of two triangles each.
            pass.draw(18, pins);
        }
    }

    private write(r: Resources): void {
        const { gpu } = r, { device } = gpu;
        const pins = this.marker ? [...this.pins, this.marker] : this.pins;
        // The bindings take at least one point, pin and badge.
        const points = this.points.length ? this.points : new Uint32Array(POINT_WORDS);
        const words = new Int32Array(Math.max(pins.length, 1) * PIN_WORDS), layers = Math.max(this.pins.length, 1);
        if (this.fresh.points) {
            if (!r.points || r.points.size < points.byteLength) {
                if (r.points) gpu.destroy('route', r.points);
                r.points = gpu.buffer('route', points.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
            }
            device.queue.writeBuffer(r.points, 0, points);
        }
        if (this.fresh.pins) {
            // x, y, the colour, and the layer of the badge plus one, or 0 for a sphere.
            pins.forEach(({ coordinate, colour, badge }, k) => words.set([...worldPixel(coordinate, UNITS).map(Math.round), packed(colour), badge ? k + 1 : 0], k * PIN_WORDS));
            if (!r.pins || r.pins.size < words.byteLength) {
                if (r.pins) gpu.destroy('route', r.pins);
                r.pins = gpu.buffer('route', words.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
            }
            device.queue.writeBuffer(r.pins, 0, words);
        }
        if (this.fresh.badges) {
            if (r.badges?.depthOrArrayLayers !== layers) {
                if (r.badges) gpu.destroy('route', r.badges);
                r.badges = gpu.texture('route', {
                    size: [BADGE_SIZE, BADGE_SIZE, layers], format: 'rgba8unorm',
                    usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
                }, BADGE_SIZE * BADGE_SIZE * 4 * layers);
            }
            const badges = r.badges;
            this.pins.forEach(({ badge }, k) => badge && device.queue.copyExternalImageToTexture(
                { source: badge }, { texture: badges, origin: [0, 0, k], premultipliedAlpha: true }, [BADGE_SIZE, BADGE_SIZE]));
        }
        if (this.fresh.bold) device.queue.writeBuffer(r.bold, 0, new Uint32Array([this.bold ?? 0]));
        this.fresh = { points: false, pins: false, badges: false, bold: false };
        // The shader counts the points by the bound size, so a shorter route binds less of the buffer.
        r.group = device.createBindGroup({ layout: r.layout, entries: [
            { binding: 0, resource: { buffer: r.frame } }, { binding: 1, resource: { buffer: r.points!, size: points.byteLength } },
            { binding: 2, resource: { buffer: r.pins! } }, { binding: 3, resource: r.badges!.createView({ dimension: '2d-array' }) },
            { binding: 4, resource: r.sampler }, { binding: 5, resource: { buffer: r.bold } },
        ] });
    }
}

async function routePipelines(gpu: Gpu) {
    const { device } = gpu, { surface, features } = gpu.once(layouts), { VERTEX, FRAGMENT } = GPUShaderStage;
    const layout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: VERTEX | FRAGMENT, buffer: { type: 'uniform' } },
        { binding: 1, visibility: VERTEX, buffer: { type: 'read-only-storage' } },
        { binding: 2, visibility: VERTEX | FRAGMENT, buffer: { type: 'read-only-storage' } },
        { binding: 3, visibility: FRAGMENT, texture: { viewDimension: '2d-array' } },
        { binding: 4, visibility: FRAGMENT, sampler: {} },
        { binding: 5, visibility: VERTEX, buffer: { type: 'uniform' } },
    ] });
    const empty = device.createBindGroupLayout({ entries: [] }), module = device.createShaderModule({ label: 'route', code: wgsl('route/route.wgsl') });
    const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [layout, surface, empty, features] });
    // Premultiplied: what is drawn covers by its alpha and adds the rest of its light.
    const blend: GPUBlendComponent = { srcFactor: 'one', dstFactor: 'one-minus-src-alpha' };
    const pipeline = (name: 'route' | 'pin', topology: GPUPrimitiveTopology, hidden = false) => device.createRenderPipelineAsync({
        label: name, layout: pipelineLayout,
        vertex: { module, entryPoint: `${name}_vertex` },
        fragment: {
            module, entryPoint: `${name}_fragment`, targets: [{ format: sceneFormat(gpu), blend: { color: blend, alpha: blend } }],
            constants: hidden ? { SHOWN: HIDDEN_PIN } : {},
        },
        primitive: { topology },
        depthStencil: { format: DEPTH, depthWriteEnabled: false, depthCompare: hidden ? 'less-equal' : 'greater' },
    });
    const [line, ...pin] = await Promise.all([pipeline('route', 'triangle-strip'), pipeline('pin', 'triangle-list'), pipeline('pin', 'triangle-list', true)]);
    return { line, pin, layout, empty };
}
