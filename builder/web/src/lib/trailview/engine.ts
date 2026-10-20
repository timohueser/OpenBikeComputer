import type { PlannerConfig } from '../planner/config';
import { instantAt, solarPosition, solarTerms } from '../planner/layers/sun';
import { eyeAltitude, move, viewProjection, type Pose } from './frame/camera';
import { Controls } from './frame/controls';
import { CURVATURE, frameAt, type Frame } from './frame/frame';
import { Gpu } from './gpu/device';
import { PassTimes } from './gpu/timing';
import type { Labs } from './labs';
import { NaiveTerrain, terrariumTiles, type Eye, type Place } from './terrain/naive';
import { DEBUG_VIEWS, TIME_ZONE, type View, type Weather } from './view';

const PASSES = ['terrain'];
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
    cpu: number;
    /** Bytes per GPU pool. */
    pools: Map<string, number>;
    requests: number;
    bytes: number;
    failed: number;
    drawn: number;
    resident: number;
    recoveries: number;
}

/** The engine: a frame loop that draws a view on demand, and recovers from a lost device. */
export class TrailView {
    /** Called with the view after it changes. */
    onView?: (view: View) => void;
    recoveries = 0;
    private readonly context: GPUCanvasContext;
    private readonly frame: Frame;
    private readonly terrain: NaiveTerrain;
    private readonly controls: Controls;
    private pose: Pose;
    private extras: Pick<View, 'date' | 'time' | 'weather' | 'debug'>;
    private gpuState!: { gpu: Gpu; uniform: GPUBuffer; times: PassTimes; depth?: GPUTexture };
    /** The last ground under the eye that a tile knew; a new view forgets it. */
    private held?: number;
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
        const terrains = { naive: () => new NaiveTerrain(this.frame, terrariumTiles(config.terrain), config.bounds, () => this.request()) };
        this.terrain = terrains[labs.terrain]();
        this.controls = new Controls(canvas, {
            pose: () => this.pose,
            moved: pose => { this.pose = pose; this.moved = true; this.request(); },
            ground: () => this.ground(),
        });
        new ResizeObserver(() => this.request()).observe(canvas);
    }

    /** Starts the tile loads, and compiles the pipelines while they run. */
    static async start(canvas: HTMLCanvasElement, gpu: Gpu, config: PlannerConfig, view: View, labs: Labs): Promise<TrailView> {
        const engine = new TrailView(canvas, config, view, labs);
        engine.terrain.select(engine.place());
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
        this.pose = this.poseOf(view);
        this.extras = { date: view.date, time: view.time, weather: view.weather, debug: view.debug };
        this.held = undefined;
        this.moved = true;
        this.request();
    }

    stats(): Stats {
        const { gpu, times } = this.gpuState;
        return {
            adapter: gpu.adapter, gpu: { ...times.ms }, cpu: this.cpu, pools: new Map(gpu.pools), requests: this.terrain.requests,
            bytes: this.terrain.bytes, failed: this.terrain.failed, drawn: this.drawn, resident: this.terrain.resident, recoveries: this.recoveries,
        };
    }

    /** Resolves after the next frame is drawn. */
    nextFrame(): Promise<void> {
        this.request();
        return new Promise(resolve => this.rendered.push(resolve));
    }

    /** Resolves after a frame in which no tile loads and every wanted tile is drawn, once the GPU has finished it. */
    async waitForIdle(): Promise<void> {
        await this.nextFrame();
        // A tile that arrives draws the next frame.
        while (!this.terrain.idle) await new Promise<void>(resolve => this.rendered.push(resolve));
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

    /** The ground under the eye; where no tile knows it, the last known ground. */
    private ground(): number | undefined {
        const height = this.terrain.heightAt(...this.frame.lonLat(this.pose.east, this.pose.north));
        if (height !== undefined) this.held = height;
        return this.held;
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
        const steer = this.controls.steer();
        if (steer) this.pose = move(this.pose, steer, this.last ? Math.min(0.1, (now - this.last) / 1000) : 0, this.ground());
        this.last = steer ? now : 0;
        const eye = this.eye(), draws = this.terrain.select(this.place());
        const { gpu, uniform, times } = this.gpuState;
        const target = this.target(gpu);
        if (eye) gpu.device.queue.writeBuffer(uniform, 0, this.uniforms(eye, target.width / target.height));
        const encoder = gpu.device.createCommandEncoder();
        const weather = WEATHER[this.extras.weather];
        const pass = encoder.beginRenderPass({
            colorAttachments: [{
                view: this.context.getCurrentTexture().createView(), loadOp: 'clear', storeOp: 'store',
                clearValue: { r: weather.horizon[0] / 255, g: weather.horizon[1] / 255, b: weather.horizon[2] / 255, a: 1 },
            }],
            depthStencilAttachment: { view: target.createView(), depthClearValue: 0, depthLoadOp: 'clear', depthStoreOp: 'discard' },
            timestampWrites: times.writes('terrain'),
        });
        if (eye) this.terrain.draw(pass, draws, eye);
        pass.end();
        const read = times.resolve(encoder);
        gpu.device.queue.submit([encoder.finish()]);
        read();
        this.cpu = performance.now() - start;
        this.drawn = eye ? draws.length : 0;
        if (steer) this.request();
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

    private uniforms(eye: Eye, aspect: number): ArrayBuffer {
        const data = new ArrayBuffer(FRAME_BYTES), floats = new Float32Array(data);
        const weather = WEATHER[this.extras.weather];
        floats.set(viewProjection(this.pose.heading, this.pose.pitch, aspect), 0);
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
