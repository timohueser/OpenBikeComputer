import type { PlannerConfig } from '../planner/config';
import { instantAt, solarPosition, solarTerms } from '../planner/layers/sun';
import { eyeAltitude, follow, groundOf, move, viewProjection, type Footing, type Pose } from './frame/camera';
import { Controls } from './frame/controls';
import { CURVATURE, frameAt, type Frame } from './frame/frame';
import { Gpu } from './gpu/device';
import { PassTimes } from './gpu/timing';
import type { Labs } from './labs';
import { featureTiles, pageDecoder, releaseTiles } from './stream/terrain';
import { Terrain, type Audit, type TerrainStats } from './terrain/terrain';
import { DEBUG_VIEWS, TIME_ZONE, type View, type Weather } from './view';

const PASSES = ['terrain', 'stamp'];
/** Metres to a view that is at another place. */
const NEW_PLACE = 1000;
/** The clear colour of the crack test: any red pixel is a hole in the terrain. */
const RED = { r: 1, g: 0, b: 0, a: 1 };
const DEPTH: GPUTextureFormat = 'depth32float';
/** `Frame` in terrain.wgsl. */
const FRAME_BYTES = 112;
const MAX_PIXEL_RATIO = 2;
/** The visibility at sea level in clear weather. */
const HAZE_M = 35_000;
/** From the prototype's weather presets: the sunlight, the visibility as a share of HAZE_M, and the horizon. */
const WEATHER: Record<Weather, { sun: number; haze: number; horizon: [number, number, number] }> = {
    sunny: { sun: 1, haze: 1, horizon: [0xb4, 0xbc, 0xc4] },
    cloudy: { sun: 0.95, haze: 0.7, horizon: [0xb4, 0xbc, 0xc4] },
    rainy: { sun: 0.6, haze: 0.3, horizon: [0x87, 0x8e, 0x95] },
};
const RAD = Math.PI / 180;

export interface Stats {
    adapter: string;
    /** Milliseconds per render pass, from timestamp queries. */
    gpu: Record<string, number>;
    /** The stamp jobs of the frame whose times `gpu` holds, by kind, and that frame's start in milliseconds. */
    timed?: Record<string, number>;
    cpu: number;
    /** Bytes per GPU pool. */
    pools: Map<string, number>;
    requests: number;
    bytes: number;
    failed: number;
    /** Terrain nodes drawn. */
    drawn: number;
    /** Terrain pages on the GPU. */
    resident: number;
    recoveries: number;
    terrain: TerrainStats;
}

/** Where the selection looks from: `above` is the eye's height above the ground, or the pose's height over an unknown ground. */
interface Place { east: number; north: number; above: number }
/** The eye in the frame, at height z above sea level. */
interface Eye extends Place { z: number }

/** The engine: a frame loop that draws a view on demand, and recovers from a lost device. */
export class TrailView {
    /** Called with the view after it changes. */
    onView?: (view: View) => void;
    recoveries = 0;
    private readonly context: GPUCanvasContext;
    private readonly frame: Frame;
    private readonly terrain: Terrain;
    private readonly controls: Controls;
    private pose: Pose;
    private extras: Pick<View, 'date' | 'time' | 'weather' | 'debug'>;
    private gpuState!: { gpu: Gpu; uniform: GPUBuffer; times: PassTimes; depth?: GPUTexture };
    /** The ground under the eye, on the plane of the last probe; a new view forgets it. */
    private footing?: Footing;
    private ready = false;
    private pending = false;
    private moved = false;
    private last = 0;
    private cpu = 0;
    private drawn = 0;
    private rendered: (() => void)[] = [];

    private constructor(private readonly canvas: HTMLCanvasElement, config: PlannerConfig, view: View, labs: Labs) {
        this.context = canvas.getContext('webgpu')!;
        this.frame = frameAt(view.lon, view.lat);
        this.pose = this.poseOf(view);
        this.extras = { date: view.date, time: view.time, weather: view.weather, debug: view.debug };
        this.terrain = new Terrain(this.frame, { fetch: releaseTiles(config), decode: pageDecoder(labs.pages) }, featureTiles(config), config.bounds, labs, () => this.request());
        this.controls = new Controls(canvas, {
            pose: () => this.pose,
            moved: pose => { this.pose = pose; this.moved = true; this.request(); },
            ground: () => this.ground(),
        });
        new ResizeObserver(() => this.request()).observe(canvas);
    }

    /** Compiles the pipelines, then starts the tile loads. */
    static async start(canvas: HTMLCanvasElement, gpu: Gpu, config: PlannerConfig, view: View, labs: Labs): Promise<TrailView> {
        const engine = new TrailView(canvas, config, view, labs);
        await engine.attach(gpu);
        engine.request();
        return engine;
    }

    get gpu(): Gpu {
        return this.gpuState.gpu;
    }

    get view(): View {
        const [lon, lat] = this.frame.lonLat(this.pose.east, this.pose.north);
        const heading = ((this.pose.heading - this.convergence(this.pose.east, this.pose.north)) % 360 + 360) % 360;
        return { lat, lon, heading, pitch: this.pose.pitch, mode: this.pose.mode, height: this.pose.height, ...this.extras };
    }

    set view(view: View) {
        const pose = this.poseOf(view);
        // A view at another place forgets the ground; a scripted flight step keeps it for the probe to follow.
        if (Math.hypot(pose.east - this.pose.east, pose.north - this.pose.north) > NEW_PLACE) {
            this.footing = undefined;
            this.terrain.forget();
        }
        this.pose = pose;
        this.extras = { date: view.date, time: view.time, weather: view.weather, debug: view.debug };
        this.moved = true;
        this.request();
    }

    stats(): Stats {
        const { gpu, times } = this.gpuState, { pages } = this.terrain, terrain = this.terrain.stats;
        return {
            adapter: gpu.adapter, gpu: { ...times.ms }, timed: times.tag, cpu: this.cpu, pools: new Map(gpu.pools), requests: pages.requests, bytes: pages.bytes, failed: pages.failed + this.terrain.stamps.failed,
            drawn: this.drawn, resident: terrain.resident.stored + terrain.resident.synth, recoveries: this.recoveries, terrain,
        };
    }

    /** drawnHeight at points from the probe and from an instance shader, with the state of the last frame. */
    check(points: [number, number][]): Promise<{ probe: number[]; instance: number[] }> {
        return this.terrain.check(points);
    }

    /** The invariant audit of the drawn terrain, with the state of the last frame: call it after `waitForIdle()`. */
    audit(): Promise<Audit> {
        return this.terrain.audit();
    }

    /** Resolves after the next frame is drawn. */
    nextFrame(): Promise<void> {
        this.request();
        return new Promise(resolve => this.rendered.push(resolve));
    }

    /**
     * Resolves after a frame in which no tile loads, every wanted page is drawn and the eye stands on
     * the probed ground, once the GPU has finished it.
     */
    async waitForIdle(): Promise<void> {
        await this.nextFrame();
        // A tile or a probe that arrives draws the next frame.
        while (!this.terrain.idle || !this.settled()) await new Promise<void>(resolve => this.rendered.push(resolve));
        await this.gpu.device.queue.onSubmittedWorkDone();
    }

    private async attach(gpu: Gpu): Promise<void> {
        this.context.configure({ device: gpu.device, format: gpu.format, alphaMode: 'opaque' });
        const uniform = gpu.buffer('uniforms', FRAME_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        this.gpuState = { gpu, uniform, times: new PassTimes(gpu, PASSES) };
        await this.terrain.attach(gpu, uniform, DEPTH);
        // A lost device is created again and gets every pool again, without a reload of the page.
        gpu.device.lost.then(async info => {
            console.warn(`The WebGPU device was lost (${info.reason}): ${info.message}`);
            this.ready = false;
            const next = await Gpu.request();
            if (!next) return console.error('WebGPU has no adapter after a lost device.');
            await this.attach(next);
            this.recoveries++;
            this.request();
        });
        this.ready = true;
    }

    private request(): void {
        if (this.pending) return;
        this.pending = true;
        requestAnimationFrame(now => this.render(now));
    }

    /** The ground under the eye, from the GPU probe; where no level knows it, the last known ground. */
    private ground(): number | undefined {
        return this.footing && groundOf(this.footing, this.pose.east, this.pose.north);
    }

    /** Whether the eye stands on the last probe, with no step left to fade out. */
    private settled(): boolean {
        const probe = this.terrain.ground;
        return !probe || Number.isNaN(probe.height) || (probe.frame === this.footing?.probe.frame && this.footing.step === 0);
    }

    /** The eye; a walking eye waits for a known ground. */
    private eye(): Eye | undefined {
        const ground = this.ground(), z = eyeAltitude(this.pose, ground);
        if (z === undefined) return undefined;
        return { east: this.pose.east, north: this.pose.north, z, above: ground === undefined ? this.pose.height : z - ground };
    }

    /** Where the tiles are selected from; over an unknown ground, the pose's height stands for the height above it. */
    private place(): Place {
        return this.eye() ?? { east: this.pose.east, north: this.pose.north, above: this.pose.height };
    }

    /** Degrees clockwise from the frame's north to true north at a point. */
    private convergence(east: number, north: number): number {
        const [lon, lat] = this.frame.lonLat(east, north), [e, n] = this.frame.local(lon, lat + 1e-4);
        return Math.atan2(e - east, n - north) / RAD;
    }

    private poseOf(view: View): Pose {
        const [east, north] = this.frame.local(view.lon, view.lat);
        return { east, north, heading: view.heading + this.convergence(east, north), pitch: view.pitch, mode: view.mode, height: view.height };
    }

    private render(now: number): void {
        this.pending = false;
        if (!this.ready) return;
        const start = performance.now();
        const steer = this.controls.steer(), dt = this.last ? Math.min(0.1, (now - this.last) / 1000) : 0;
        if (steer) this.pose = move(this.pose, steer, dt, this.ground());
        const probe = this.terrain.ground;
        if (probe && !Number.isNaN(probe.height)) {
            const [east, north] = this.frame.local(...probe.lonLat);
            this.footing = follow(this.footing, { ...probe, east, north }, this.pose.east, this.pose.north, dt);
        }
        const settled = this.settled();
        this.last = steer || !settled ? now : 0;
        const eye = this.eye(), place = this.place();
        const { gpu, uniform, times } = this.gpuState;
        const target = this.target(gpu), aspect = target.width / target.height;
        const viewProj = viewProjection(this.pose.heading, this.pose.pitch, aspect);
        this.terrain.update({ lonLat: this.frame.lonLat(place.east, place.north), above: place.above, z: eye?.z, viewProj, pixels: target.height });
        if (eye) gpu.device.queue.writeBuffer(uniform, 0, this.uniforms(eye, viewProj));
        const encoder = gpu.device.createCommandEncoder();
        const probed = this.terrain.encode(encoder, pass => times.writes(pass));
        const weather = WEATHER[this.extras.weather];
        const pass = encoder.beginRenderPass({
            colorAttachments: [{
                view: this.context.getCurrentTexture().createView(), loadOp: 'clear', storeOp: 'store',
                clearValue: this.extras.debug === 'cracks' ? RED : { r: weather.horizon[0] / 255, g: weather.horizon[1] / 255, b: weather.horizon[2] / 255, a: 1 },
            }],
            depthStencilAttachment: { view: target.createView(), depthClearValue: 0, depthLoadOp: 'clear', depthStoreOp: 'discard' },
            timestampWrites: times.writes('terrain'),
        });
        if (eye) this.terrain.draw(pass);
        pass.end();
        const read = times.resolve(encoder, { ...this.terrain.stats.jobs, frame: start });
        gpu.device.queue.submit([encoder.finish()]);
        read();
        probed();
        this.cpu = performance.now() - start;
        this.drawn = eye ? this.terrain.drawn : 0;
        // Frames go on while the eye moves or settles on the ground, and while the terrain has work that no load waits for.
        if (steer || !settled || this.terrain.working) this.request();
        if (steer || this.moved) this.onView?.(this.view);
        this.moved = false;
        for (const resolve of this.rendered.splice(0)) resolve();
    }

    /** The depth target at the canvas size, which follows the element. */
    private target(gpu: Gpu): GPUTexture {
        const ratio = Math.min(devicePixelRatio, MAX_PIXEL_RATIO);
        const width = Math.max(1, Math.round(this.canvas.clientWidth * ratio)), height = Math.max(1, Math.round(this.canvas.clientHeight * ratio));
        let { depth } = this.gpuState;
        if (depth?.width === width && depth.height === height) return depth;
        if (depth) gpu.destroy('targets', depth);
        this.canvas.width = width;
        this.canvas.height = height;
        depth = gpu.texture('targets', { size: [width, height], format: DEPTH, usage: GPUTextureUsage.RENDER_ATTACHMENT }, 4 * width * height);
        this.gpuState.depth = depth;
        return depth;
    }

    private uniforms(eye: Eye, viewProj: Float32Array): ArrayBuffer {
        const data = new ArrayBuffer(FRAME_BYTES), floats = new Float32Array(data);
        const weather = WEATHER[this.extras.weather];
        floats.set(viewProj, 0);
        floats.set(this.sun(), 16);
        floats.set([...weather.horizon.map(c => c / 255), HAZE_M * weather.haze, eye.z, CURVATURE], 20);
        new Uint32Array(data)[26] = DEBUG_VIEWS.indexOf(this.extras.debug);
        return data;
    }

    /** The direction to the sun in the frame, and the sunlight, at the view's place, date and time. */
    private sun(): number[] {
        const { east, north } = this.pose, [lon, lat] = this.frame.lonLat(east, north);
        const [hours, minutes] = this.extras.time.split(':').map(Number), minute = 60 * hours + minutes;
        let instant: number;
        try {
            instant = instantAt(this.extras.date, minute, TIME_ZONE);
        } catch {
            // The clock skips this time when it changes to summer time.
            instant = instantAt(this.extras.date, minute + 60, TIME_ZONE);
        }
        const { bearing, altitude } = solarPosition([lon, lat], solarTerms(instant));
        const b = bearing + this.convergence(east, north) * RAD;
        const light = WEATHER[this.extras.weather].sun * Math.min(1, Math.max(0, (altitude + 0.05) / 0.15));
        return [Math.sin(b) * Math.cos(altitude), Math.cos(b) * Math.cos(altitude), Math.sin(altitude), light];
    }
}
