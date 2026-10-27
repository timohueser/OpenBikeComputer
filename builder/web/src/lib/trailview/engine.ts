import type { PlannerConfig } from '../planner/config';
import { instantAt, solarPosition, solarTerms } from '../planner/layers/sun';
import { eyeAltitude, follow, groundOf, MIN_CLEARANCE, move, VFOV, viewProjection, type Footing, type Pose } from './frame/camera';
import { Controls } from './frame/controls';
import { CURVATURE, frameAt, type Frame } from './frame/frame';
import { Gpu } from './gpu/device';
import { PassTimes } from './gpu/timing';
import { Ground, groundPipelines, type GroundStats } from './ground/ground';
import { moonAt, starFrame, type Mat3 } from './light/celestial';
import { airColour, eyeLook, lightAt, moonlit, moonShare, weatherLight, WEATHERS, type EyeLook } from './light/light';
import { LOOK_BYTES, lookUniform } from './light/look';
import { Shadow } from './light/shadow';
import { DARK_SKY, nightSky } from './light/night';
import { Sky, SKY_CREDITS, skyPipeline } from './light/sky';
import { Resolve, resolvePipeline } from './resolve/resolve';
import { MAX_SCALE, Targets } from './resolve/targets';
import { jitter, STILL_FRAMES, Temporal, type Step } from './resolve/temporal';
import { EYE, Instances, SUN, type InstanceStats, type LastFrame } from './instances/instances';
import { GroundSource } from './stream/ground';
import { eyeTiles, pageDecoder, type EyeTiles } from './stream/terrain';
import { Terrain, terrainPipelines, type Audit, type TerrainStats } from './terrain/terrain';
import { Houses, type HouseStats } from './structures/houses';
import { Forest } from './vegetation/forest';
import { TreeModels } from './vegetation/tree-models';
import { Orchards } from './vegetation/orchards';
import { Plants } from './vegetation/plants';
import { Dressing } from './dressing/dressing';
import { KindPoints } from './structures/kind-points';
import { Pacer } from './pacer';
import { DEBUG_VIEWS, TIME_ZONE, type View } from './view';

const PASSES = ['terrain', 'ground', 'stamp', 'houses', 'vegetation', 'casters', 'shadow', 'far-casters', 'far-shadow', 'mips', 'resolve'];
/** Metres to a view that is at another place. */
const NEW_PLACE = 1000;
/** A step of the footing this small no longer moves the view about the eye: `eyeReady` counts it as settled. */
const SETTLED_M = 0.02;
/** The longest that the other loads wait for the tiles under the eye: a tile that hangs must not hold the view back until its request times out. */
const EYE_WAIT_MS = 1000;
/** The clear colour of the crack test: any red pixel is a hole in the terrain. */
const RED = { r: 1, g: 0, b: 0, a: 1 };
/** `Frame` in terrain.wgsl. */
const FRAME_BYTES = 96;
const MAX_PIXEL_RATIO = 2;
const RAD = Math.PI / 180;
/** A walking eye steps up or down this far onto a floor; on a floor this far under the terrain it is in a tunnel. */
const STEP_M = 1, HEADROOM_M = 2;
/** A flying eye under the terrain is in a tunnel when a floor lies at most this far below it. */
const BORE_M = 6;

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
    /** The render scale: render pixels per output pixel along each axis. */
    scale: number;
    /** Terrain pages on the GPU. */
    resident: number;
    recoveries: number;
    terrain: TerrainStats;
    ground: GroundStats;
    houses: HouseStats;
    instances: InstanceStats;
}

/**
 * The pipelines of the first frame: the terrain and its compute, the ground pages, the sky and the
 * resolve. The other passes compile theirs at their attach, after the first frame.
 */
const FIRST: ((gpu: Gpu) => Promise<unknown>)[] = [terrainPipelines, groundPipelines, skyPipeline, resolvePipeline];
/** The longest that the other passes wait for the first frame: a view whose eye has no ground draws none. */
const REST_WAIT_MS = 2000;

/** Where the eye stands: a height, and whether it lies in a tunnel. */
interface Feet { z: number; tunnel: boolean; floor: boolean }

/** Where the selection looks from: `above` is the eye's height above the ground, or the pose's height over an unknown ground. */
interface Place { east: number; north: number; above: number }
/** The eye in the frame, at height z above sea level. */
interface Eye extends Place { z: number }

/** The engine: a frame loop that draws a view on demand, and recovers from a lost device until `destroy()`. */
export class TrailView {
    /** Called with the view after it changes. */
    onView?: (view: View) => void;
    recoveries = 0;
    /** Picks the display frames that draw: `pacer.rate` is the frame rate. */
    readonly pacer = new Pacer();
    private readonly context: GPUCanvasContext;
    private readonly frame: Frame;
    private readonly terrain: Terrain;
    private readonly groundLook: Ground;
    private readonly houses: Houses;
    private readonly instances = new Instances();
    private readonly trees: TreeModels;
    private readonly forest: Forest;
    private readonly plants: Plants;
    private readonly orchards: Orchards;
    private readonly kindPoints: KindPoints;
    private readonly dressing: Dressing;
    private readonly sky = new Sky();
    private readonly shadow = new Shadow();
    private readonly resolve = new Resolve();
    private readonly temporal = new Temporal();
    private readonly controls: Controls;
    private destroyed = false;
    private pose: Pose;
    private extras: Pick<View, 'date' | 'time' | 'weather' | 'debug'>;
    private gpuState!: { gpu: Gpu; uniform: GPUBuffer; look: GPUBuffer; times: PassTimes; targets: Targets };
    /** The ground under the eye, on the plane of the last probe; a new view forgets it. */
    private footing?: Footing;
    /** Where the eye stood in the last frame: the height, and whether it was in a tunnel. */
    private feet?: Feet;
    private ready = false;
    /** No frame draws before `start` lets it: a frame asks for every tile, and the tiles under the eye come first. */
    private held = true;
    /** The attach of the other passes, which starts at the first drawn frame (`firstDrawn`); it rejects when one fails. */
    private rest: Promise<unknown> = Promise.resolve();
    private firstDrawn?: () => void;
    /** Whether every pass is attached; `completed` until a frame draws them all. */
    private complete = false;
    private completed = false;
    private pending = false;
    private moved = false;
    private last = 0;
    private cpu = 0;
    private drawn = 0;
    /** Whether the last frame could draw: the eye has ground and the ground look is ready. */
    private drawing = false;
    /** What the last frame drew: the camera and the extras, to tell a move and a change. */
    private camera: number[] = [];
    private drawnExtras = '';
    private rendered: (() => void)[] = [];
    /** The last frame that drew, for occlusion culling: its depth, pixels and view projection, and its eye. */
    private lastFrame?: Omit<LastFrame, 'moved'> & { eye: number[] };
    /** Whether the last frame that placed the instances moved or changed: the next one places them again, culled by a depth of the same view. */
    private unrest = true;
    /**
     * The brightness of the sky at the eye on a moonless night, in magnitudes per square arcsecond, as
     * a sky quality meter reads it: about 22 far from lights, 18 in a town. It sets the faintest
     * stars and the Milky Way's share (light/night.ts). Nothing sets it from data yet.
     */
    private skyMagnitude = DARK_SKY;

    private constructor(private readonly canvas: HTMLCanvasElement, config: PlannerConfig, view: View, ground: GroundSource | undefined, tiles: EyeTiles, private readonly stop: AbortController, private readonly scale: number) {
        // `destroy()` aborts it: it ends the tile requests, the workers and the listeners.
        const { signal } = stop;
        this.context = canvas.getContext('webgpu')!;
        this.frame = frameAt(view.lon, view.lat);
        this.pose = this.poseOf(view);
        this.extras = { date: view.date, time: view.time, weather: view.weather, debug: view.debug };
        const { features } = tiles;
        this.terrain = new Terrain(this.frame, { fetch: tiles.terrain, decode: pageDecoder(signal) }, features, config.bounds, () => this.request(), signal);
        this.groundLook = new Ground(ground, () => this.request());
        this.houses = new Houses(this.frame, features, this.groundLook.rules, () => this.request(), signal);
        this.trees = new TreeModels(this.instances, () => this.request(), () => this.treesNear(), signal);
        this.forest = new Forest(config, this.groundLook.rules, this.trees, this.instances, () => this.request(), signal);
        this.plants = new Plants(this.groundLook.rules, this.trees, this.instances, () => this.request(), signal);
        this.orchards = new Orchards(this.groundLook.rules, this.trees, this.instances, features, () => this.request(), signal);
        this.kindPoints = new KindPoints(this.instances);
        this.dressing = new Dressing(this.groundLook.rules, this.instances);
        this.controls = new Controls(canvas, {
            pose: () => this.pose,
            moved: pose => { this.pose = pose; this.moved = true; this.request(); },
            ground: () => this.ground(),
        }, signal);
        const resize = new ResizeObserver(() => this.request());
        resize.observe(canvas);
        signal.addEventListener('abort', () => resize.disconnect());
    }

    /**
     * Compiles the pipelines of the first frame on a device, without a view and without pools: on a cold
     * shader cache that takes seconds. Call it as soon as a view is likely, such as when a drag toward one
     * starts; `start` on the same device takes what it compiled, also while it still compiles.
     */
    static prepare(gpu: Gpu): Promise<void> {
        return Promise.all(FIRST.map(make => gpu.once(make))).then(() => {});
    }

    /**
     * Starts the terrain and feature tiles under the eye, reads the ruleset and opens the ground layers,
     * attaches what the first frame draws, then starts the other tile loads once the tiles under the eye
     * are in, or after EYE_WAIT_MS: on a slow link the other loads would share it with them. The other
     * passes draw once they attach after the first frame. `scale` is the render scale.
     */
    static async start(canvas: HTMLCanvasElement, gpu: Gpu, config: PlannerConfig, view: View, scale: number): Promise<TrailView> {
        const stop = new AbortController(), tiles = eyeTiles(config, [view.lon, view.lat], stop.signal);
        const ground = await GroundSource.open(config, view.date, stop.signal).catch(error => void console.warn(`TrailView draws no ground: ${error instanceof Error ? error.message : error}`));
        const engine = new TrailView(canvas, config, view, ground ?? undefined, tiles, stop, scale);
        // No caller holds an engine whose start fails, so it ends itself: its workers and listeners run already.
        await engine.attach(gpu).catch(error => {
            engine.destroy();
            throw error;
        });
        await Promise.race([tiles.arrived, new Promise(resolve => setTimeout(resolve, EYE_WAIT_MS))]);
        engine.held = false;
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
            this.feet = undefined;
            this.terrain.forget();
            this.temporal.reset();
            this.lastFrame = undefined;
        }
        this.pose = pose;
        this.extras = { date: view.date, time: view.time, weather: view.weather, debug: view.debug };
        this.moved = true;
        this.request();
    }

    get skyBrightness(): number {
        return this.skyMagnitude;
    }

    set skyBrightness(magnitude: number) {
        this.skyMagnitude = magnitude;
        this.request();
    }

    stats(): Stats {
        const { gpu, times } = this.gpuState, { pages } = this.terrain, terrain = this.terrain.stats, ground = this.groundLook.stats, forest = this.forest.requests;
        return {
            adapter: gpu.adapter, gpu: { ...times.ms }, timed: times.tag, cpu: this.cpu, pools: new Map(gpu.pools),
            requests: pages.requests + ground.requests + forest.requests, bytes: pages.bytes + ground.bytes + forest.bytes, failed: pages.failed + ground.failed + forest.failed + this.terrain.stamps.failed,
            drawn: this.drawn, scale: this.scaleNow, resident: terrain.resident.stored + terrain.resident.synth, recoveries: this.recoveries, terrain, ground, houses: this.houses.stats, instances: this.instances.stats,
        };
    }

    /** The credits of the ground layers and the sky. */
    get attribution(): string[] {
        return [...this.groundLook.attribution, SKY_CREDITS];
    }

    /** drawnHeight at points from the probe and from an instance shader, and the share of the sun there, with the state of the last frame. */
    check(points: [number, number][]): Promise<{ probe: number[]; instance: number[]; sun: number[] }> {
        return this.terrain.check(points, this.groundLook.group!);
    }

    /** The invariant audit of the drawn terrain, with the state of the last frame: call it after `waitForIdle()`. */
    audit(): Promise<Audit> {
        return this.terrain.audit();
    }

    /**
     * Ends the engine: no frame draws, no tile loads, no worker runs, and the buffers and textures that
     * it made are destroyed. The device stays for the next engine. The waits for a frame resolve.
     */
    destroy(): void {
        if (this.destroyed) return;
        this.destroyed = true;
        this.ready = false;
        this.stop.abort();
        for (const resolve of this.rendered.splice(0)) resolve();
        this.context.unconfigure();
        // A start that fails in its first attach may have no GPU state yet.
        this.gpuState?.times.destroy();
        this.gpuState?.gpu.release();
    }

    /** Resolves after the next frame is drawn, or at once after `destroy()`. */
    nextFrame(): Promise<void> {
        this.request();
        return this.afterFrame();
    }

    /** Resolves after the next frame that draws; a destroyed engine draws none. */
    private afterFrame(): Promise<void> {
        return this.destroyed ? Promise.resolve() : new Promise(resolve => this.rendered.push(resolve));
    }

    /**
     * Resolves after a frame in which no tile loads, every wanted page and shadow map is drawn, the eye
     * stands on the probed ground and the still view has all its jittered frames, once the GPU has
     * finished it. Rejects when a pass fails to attach, as `nearReady` does.
     */
    async waitForIdle(): Promise<void> {
        await this.rest;
        await this.nextFrame();
        // A tile or a probe that arrives draws the next frame.
        while (!this.destroyed && !this.idle()) await this.afterFrame();
        if (!this.destroyed) await this.gpu.device.queue.onSubmittedWorkDone();
    }

    private idle(): boolean {
        return this.complete && this.terrain.idle && this.groundLook.idle && this.houses.idle && this.trees.idle && this.forest.idle && this.plants.idle && this.orchards.idle && this.instances.idle
            && this.sky.idle && !this.shadow.pending && this.settled() && !this.converging;
    }

    /**
     * Resolves after the first drawn frame in which the near field (NEAR_M about the eye) is final: the
     * eye stands on the probe of the finest heights under it, the terrain within NEAR_M draws at its
     * wanted levels with its stamps, the ground pages, houses, trees and plants there are in, and the
     * near shadow map holds them. It waits neither for the far field nor for the jittered frames. A view
     * that never gets there, such as one whose tiles fail, resolves when it is idle.
     */
    async nearReady(): Promise<void> {
        await this.rest;
        await this.nextFrame();
        while (!this.destroyed && !this.near() && !this.idle()) await this.afterFrame();
    }

    /**
     * Resolves after the first drawn frame in which the view about the eye (EYE_M) is final: the eye
     * stands on its final ground, and the terrain and the ground look within EYE_M draw at their wanted
     * levels with their stamps. It waits for no houses, plants, shadows, water, structures or far field.
     * A view that never gets there resolves when it is idle, or when a late pass fails to attach.
     */
    async eyeReady(): Promise<void> {
        // A failed late pass keeps the view from ever being idle: the wait then ends with it.
        let failed = false;
        this.rest.catch(() => { failed = true; });
        await this.nextFrame();
        while (!this.destroyed && !failed && !this.eyeFinal() && !this.idle()) await this.afterFrame();
    }

    private eyeFinal(): boolean {
        return this.drawing && this.terrain.eye && this.footed() && this.groundLook.eyeIdle;
    }

    /**
     * Whether the step of the footing no longer moves the camera. The last centimetres of a walking eye's
     * step down fade for half a second; nobody sees them. A flying eye above its clearance does not stand on
     * the footing, so its step moves nothing.
     */
    private footed(): boolean {
        const step = Math.abs(this.footing?.step ?? 0), ground = this.ground(), z = eyeAltitude(this.pose, ground);
        if (this.pose.mode === 'fly' && ground !== undefined && z !== undefined && z - ground - MIN_CLEARANCE > step) return true;
        return this.settled(SETTLED_M);
    }

    private near(): boolean {
        return this.drawing && this.complete && this.terrain.near && this.settled() && this.groundLook.nearIdle && this.houses.nearIdle && this.trees.idle && this.forest.idle
            && this.plants.idle && this.orchards.idle && this.instances.idle && this.shadow.nearDrawn;
    }

    /**
     * Resolves when the first frame can draw. The other passes attach at the first drawn frame, or after
     * REST_WAIT_MS: their compiles would share the compiler and the CPU with the first frame. The engine's
     * buffers and textures are made in a scope of the device, which `destroy()` releases.
     */
    private async attach(base: Gpu): Promise<void> {
        const gpu = base.scope();
        // The passes' attaches report a failed compile.
        TrailView.prepare(gpu).catch(() => {});
        this.complete = false;
        this.context.configure({ device: gpu.device, format: gpu.format, alphaMode: 'opaque' });
        const uniform = gpu.buffer('uniforms', FRAME_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        const look = gpu.buffer('uniforms', LOOK_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST);
        const targets = new Targets(gpu);
        this.gpuState = { gpu, uniform, look, times: new PassTimes(gpu, PASSES, { resolve: 'terrain' }), targets };
        this.temporal.reset();
        // The maps at once: the ground's group binds them.
        const shadow = this.shadow.attach(gpu), frames = [uniform, this.shadow.frame];
        await this.terrain.attach(gpu, uniform);
        const surface = this.terrain.surface!, features = this.terrain.features!;
        await Promise.all([this.groundLook.attach(gpu, look, surface, this.shadow.entries()), this.sky.attach(gpu), this.resolve.attach(gpu)]);
        const drawn = new Promise<void>(resolve => {
            this.firstDrawn = resolve;
            setTimeout(resolve, REST_WAIT_MS);
        });
        // A device lost before the first frame leaves the other passes to the next attach.
        this.rest = drawn.then(async () => {
            if (this.destroyed || this.gpuState.gpu !== gpu) return;
            // The night sky's files load on their own: the passes and the shadows never wait for them.
            void this.sky.attachNight(gpu).then(() => {
                if (this.destroyed) gpu.release();
                else this.request();
            });
            // A destroy during these attaches released the scope before they made their buffers, also when one fails.
            try {
                await Promise.all([shadow, this.terrain.attachParts(frames), this.houses.attach(gpu, frames, surface, features), this.instances.attach(gpu, frames, { surface, features })]);
            } finally {
                if (this.destroyed) gpu.release();
            }
            if (this.destroyed || this.gpuState.gpu !== gpu) return;
            this.complete = this.completed = true;
            this.request();
        });
        this.rest.catch(error => console.error('TrailView: a pass failed to attach:', error));
        // A lost device is created again and gets every pool again, without a reload of the page.
        gpu.device.lost.then(async info => {
            if (this.destroyed || info.reason === 'destroyed') return;
            console.warn(`The WebGPU device was lost (${info.reason}): ${info.message}`);
            this.ready = false;
            const next = await Gpu.request();
            if (this.destroyed) return;
            if (!next) return console.error('WebGPU has no adapter after a lost device.');
            try {
                await this.attach(next);
            } finally {
                if (this.destroyed) this.gpuState.gpu.release();
            }
            if (this.destroyed) return;
            // The other passes attach before the next frame: until they do, they hold the lost device's resources.
            this.firstDrawn?.();
            this.recoveries++;
            this.request();
        });
        this.ready = !this.destroyed;
    }

    /**
     * Asks for a frame. Every arrival (a terrain or ground page, stamps, houses, canopy, models, plants,
     * orchards) wakes the engine only here and goes to the GPU only in a frame: after `destroy()` none does.
     */
    private request(): void {
        if (this.pending || this.destroyed) return;
        this.pending = true;
        requestAnimationFrame(now => this.render(now));
    }

    /** The height that the eye stands on. */
    private ground(): number | undefined {
        return this.stand()?.z;
    }

    /**
     * Where the eye stands: on the ground under it from the GPU probe, or where no level knows it, the
     * last known ground. A walking eye stands on the floor of a deck or a tunnel within a step of its
     * feet instead. Off a tunnel's floor, past its edge or between its segments, it keeps its feet: the
     * hill over it is no place to land. A flying eye under the terrain stands on a tunnel's floor below it.
     */
    private stand(): Feet | undefined {
        const terrain = this.footing && groundOf(this.footing, this.pose.east, this.pose.north);
        if (terrain === undefined) return terrain;
        const at = this.frame.lonLat(this.pose.east, this.pose.north), feet = this.feet?.z ?? terrain;
        const floor = this.pose.mode === 'fly'
            ? this.pose.height < terrain ? this.terrain.floorAt(at, this.pose.height - BORE_M, this.pose.height) : undefined
            : this.terrain.floorAt(at, feet - STEP_M, feet + STEP_M) ?? (this.feet?.tunnel ? feet : undefined);
        const z = floor ?? terrain;
        return { z, tunnel: z < terrain - HEADROOM_M, floor: floor !== undefined };
    }

    /** Whether a still view that can draw needs more jittered frames. */
    private get converging(): boolean {
        return this.drawing && this.temporal.converging;
    }

    /** The render scale of the view: the debug views draw every output pixel. */
    private get scaleNow(): number {
        return this.extras.debug === 'off' ? this.scale : 1;
    }

    /** Whether the eye stands on the last probe, with at most `within` metres of a step still fading. */
    private settled(within = 0): boolean {
        const probe = this.terrain.ground;
        return !probe || Number.isNaN(probe.height) || (probe.frame === this.footing?.probe.frame && Math.abs(this.footing.step) <= within);
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

    /** The tree models of the regions about the eye, each by its weight in their packs. */
    private treesNear(): Map<string, number> {
        const [lon, lat] = this.frame.lonLat(this.pose.east, this.pose.north), near = new Map<string, number>();
        for (const [region, share] of this.groundLook.regionsAt(lon, lat)) {
            for (const { model, weight } of this.groundLook.rules.regions[region]?.trees ?? []) near.set(model, (near.get(model) ?? 0) + share * weight);
        }
        return near;
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
        if (!this.ready || this.held) return;
        if (!this.pacer.due(now)) return this.request();
        const start = performance.now();
        const steer = this.controls.steer(), dt = this.last ? Math.min(0.1, (now - this.last) / 1000) : 0;
        if (steer) this.pose = move(this.pose, steer, dt, this.ground());
        const probe = this.terrain.ground;
        if (probe && !Number.isNaN(probe.height)) {
            const [east, north] = this.frame.local(...probe.lonLat);
            this.footing = follow(this.footing, { ...probe, east, north }, this.pose.east, this.pose.north, dt);
        }
        this.feet = this.stand();
        const settled = this.settled();
        this.last = steer || !settled ? now : 0;
        const eye = this.eye(), place = this.place();
        const { gpu, uniform, times, targets } = this.gpuState;
        if (this.fit()) {
            this.temporal.reset();
            this.lastFrame = undefined;
        }
        const debug = this.extras.debug !== 'off', scale = this.scaleNow, [width, height] = targets.viewport(scale), aspect = targets.width / targets.height;
        const viewProj = viewProjection(this.pose.heading, this.pose.pitch, aspect);
        const lonLat = this.frame.lonLat(place.east, place.north);
        // The shipping render scale selects the nodes, also for a debug view: it shows the LOD that ships.
        this.terrain.update({ lonLat, above: place.above, z: eye?.z, viewProj, pixels: targets.viewport(this.scale)[1] }, this.groundLook.reads);
        this.kindPoints.update(this.terrain.kindPoints);
        const { drawn, wanted, arrived, origins } = this.terrain.selected;
        this.groundLook.update(drawn, wanted, arrived, origins, this.extras.date, lonLat, this.forest.regionPages(lonLat));
        this.houses.update(lonLat, eye && { viewProj, eye: [eye.east, eye.north, eye.z] });
        this.trees.update(this.extras.date);
        this.forest.update(lonLat);
        this.plants.update(lonLat, this.extras.date, place.above);
        this.orchards.update(lonLat, (lon, lat) => [...this.groundLook.regionsAt(lon, lat)].sort((a, b) => b[1] - a[1])[0]?.[0]);
        this.dressing.update(lonLat);
        const group = this.groundLook.group;
        this.drawing = !!(eye && group);
        let step: Step | undefined, offset: [number, number] = [0, 0], ndc: [number, number] = [0, 0], moved = true, changed = false, cascade = -1;
        let lights: Lights | undefined;
        if (eye && group) {
            const camera = [...viewProj, eye.east, eye.north, eye.z, eye.above, scale], extras = JSON.stringify([this.extras, this.skyMagnitude]);
            moved = camera.some((value, i) => value !== this.camera[i]);
            changed = this.completed || this.terrain.newContent || this.groundLook.newContent || this.houses.newContent || this.trees.newContent || this.forest.newContent || this.plants.newContent || this.orchards.newContent || this.instances.newContent || this.sky.newContent || extras !== this.drawnExtras;
            this.camera = camera;
            this.drawnExtras = extras;
            this.completed = false;
            lights = this.lights();
            const light = lights.key, casters = this.houses.newContent || this.trees.newContent || this.forest.newContent || this.plants.newContent || this.orchards.newContent;
            if (this.complete) cascade = this.shadow.due([eye.east, eye.north, eye.z - eye.above], eye.above, light, { casters, reshaped: this.terrain.reshaped }, moved);
            this.shadow.write(gpu, [eye.east, eye.north, eye.z], light);
            // A new map changes the frame as new content does.
            step = this.temporal.next(moved, changed || cascade >= 0, debug ? 1 : STILL_FRAMES);
            if (step && !debug) offset = jitter(step.sample);
            // The jitter in clip space: render pixels are y down.
            ndc = [2 * offset[0] / width, -2 * offset[1] / height];
            gpu.device.queue.writeBuffer(uniform, 0, this.uniforms(eye, jittered(viewProj, ndc), 2 * Math.tan(VFOV * RAD / 2) / targets.height, light));
            const near = this.regionLook(eye);
            gpu.device.queue.writeBuffer(this.gpuState.look, 0, this.look(eye, near, lights, aspect, ndc, scale, step?.sample ?? 0));
            const { cover } = near.weather, night = lightAt(lights.sunAltitude).night;
            this.sky.write(gpu, {
                stars: lights.stars, moon: lights.moon, moonAir: lights.moonAir, moonLight: lights.share > 0,
                night: nightSky({ sun: lights.sunAltitude, moon: lights.moonAltitude, phase: lights.phase, sky: this.skyMagnitude, cover }),
                moonDisk: (1 - cover) * (0.35 + 0.65 * night), pixel: [2 / width, 2 / height],
            });
        } else this.temporal.reset();
        const encoder = gpu.device.createCommandEncoder();
        const probed = this.terrain.encode(encoder, pass => times.writes(pass));
        // On a floor the eye stands on a structure, not on the probe: a tunnel's floor lies under the terrain.
        if (this.drawing && !this.feet?.floor) this.terrain.clampEye(encoder);
        this.groundLook.encode(encoder, () => times.writes('ground'));
        const counted = this.houses.encode(encoder, this.groundLook.computeGroup, moved, changed, () => times.writes('houses'));
        const last = eye && this.lastFrame
            ? { ...this.lastFrame, moved: [eye.east - this.lastFrame.eye[0], eye.north - this.lastFrame.eye[1], eye.z - this.lastFrame.eye[2]] as [number, number, number] } : undefined;
        // The instances keep their lists while the camera and the content rest, once a frame culled them by a depth of the same view.
        const pixels = targets.viewport(this.scale)[1], placing = !!(eye && group && step) && (moved || changed || this.unrest || this.instances.due(pixels));
        if (placing) this.unrest = moved || changed || !last;
        const placed = this.vegetation(encoder, placing ? pixels : 0, last, () => times.writes('vegetation'));
        const cast = cascade >= 0 && step && eye && group && lights ? this.cast(encoder, group, eye, cascade, lights.key) : () => {};
        if (!eye || !group) {
            // Until the eye has ground nothing draws: the horizon of the weather fills the canvas.
            encoder.beginRenderPass({
                colorAttachments: [{
                    view: this.context.getCurrentTexture().createView(), loadOp: 'clear', storeOp: 'store',
                    clearValue: this.extras.debug === 'cracks' ? RED : horizon(WEATHERS[this.extras.weather].horizon),
                }],
            }).end();
        } else if (step) {
            // The frame at the render scale in the top left of the targets; a still view that has all its frames draws none.
            const pass = encoder.beginRenderPass({
                colorAttachments: [{ view: targets.scene!.createView(), loadOp: 'clear', storeOp: 'store', clearValue: this.extras.debug === 'cracks' ? RED : [0, 0, 0, 1] }],
                depthStencilAttachment: { view: targets.depth!.createView(), depthClearValue: 0, depthLoadOp: 'clear', depthStoreOp: 'store' },
                timestampWrites: times.writes('terrain'),
            });
            pass.setViewport(0, 0, width, height, 0, 1);
            // Before the terrain: the ground that the trees hide fails the depth test.
            this.instances.draw(pass, group);
            this.terrain.draw(pass, group);
            this.houses.draw(pass, group);
            if (this.extras.debug !== 'cracks') this.sky.draw(pass, group);
            pass.end();
            this.resolve.encode(encoder, targets, this.context.getCurrentTexture().createView(), group,
                { viewProj, eye: [eye.east, eye.north, eye.z], render: [width, height], jitter: offset }, { weight: step.weight, clip: step.clip, copy: debug }, times.writes('resolve'));
            targets.swap();
            this.lastFrame = { depth: targets.depth!, pixels: [width, height], viewProj: jittered(viewProj, ndc), eye: [eye.east, eye.north, eye.z] };
        }
        const read = times.resolve(encoder, { ...this.terrain.stats.jobs, frame: start });
        gpu.device.queue.submit([encoder.finish()]);
        if (step) this.firstDrawn?.();
        read();
        probed();
        counted();
        placed();
        cast();
        this.cpu = performance.now() - start;
        this.drawn = eye ? this.terrain.drawn : 0;
        // Frames go on while the eye moves or settles on the ground, while the terrain or the ground has
        // work that no load waits for, and until a still view has all its jittered frames.
        if (steer || !settled || this.terrain.working || this.groundLook.working || this.houses.working || this.shadow.pending || this.converging) this.request();
        if (steer || this.moved) this.onView?.(this.view);
        this.moved = false;
        for (const resolve of this.rendered.splice(0)) resolve();
    }

    /**
     * Encodes the producers of instances and the sort of what they stage, for a render `pixels` high (0:
     * none draws), with the last frame's depth hiding what it hid.
     */
    private vegetation(encoder: GPUCommandEncoder, pixels: number, last: LastFrame | undefined, timing: () => GPUComputePassTimestampWrites | undefined): () => void {
        const ground = this.groundLook.group;
        if (!pixels || !ground) return () => {};
        this.instances.begin(EYE, pixels);
        this.instances.occlude(encoder, last);
        this.instances.bake(encoder);
        const pass = encoder.beginComputePass({ label: 'vegetation', timestampWrites: timing() });
        this.instances.run(pass, ground, EYE);
        // The anchored meshes' heights share the pass: a pass of their own costs more than their work.
        this.terrain.encodeAnchors(pass);
        pass.end();
        return this.instances.read(encoder);
    }

    /**
     * Draws a cascade's shadow map: the producers and the houses cull for the sun's view, then the
     * casters draw. Call the returned function after the submit.
     */
    private cast(encoder: GPUCommandEncoder, group: GPUBindGroup, eye: Eye, cascade: number, light: number[]): () => void {
        const { gpu, times } = this.gpuState, computeGroup = this.groundLook.computeGroup, label = PASSES[PASSES.indexOf('casters') + 2 * cascade];
        const at: [number, number, number] = [eye.east, eye.north, eye.z], viewProj = this.shadow.matrix(cascade, at);
        // The sun's view culls nothing by its size on screen.
        gpu.device.queue.writeBuffer(this.shadow.frame, 0, this.uniforms(eye, viewProj, 1e-6, light));
        this.instances.begin(SUN, 0, this.shadow.run(cascade));
        const pass = encoder.beginComputePass({ label, timestampWrites: times.writes(label) });
        this.instances.run(pass, group, SUN);
        if (computeGroup) this.houses.encodeCasters(pass, computeGroup, { viewProj, eye: at });
        pass.end();
        const shadowLabel = PASSES[PASSES.indexOf('casters') + 2 * cascade + 1];
        const casters = this.shadow.pass(encoder, cascade, times.writes(shadowLabel));
        this.instances.cast(casters);
        this.houses.cast(casters);
        this.terrain.cast(casters);
        casters.end();
        this.shadow.encodeMips(encoder, cascade, times.writes('mips'));
        return this.houses.readCasters(encoder);
    }

    /** Fits the canvas and the targets to the element; true when the targets are new. */
    private fit(): boolean {
        const ratio = Math.min(devicePixelRatio, MAX_PIXEL_RATIO);
        const width = Math.max(1, Math.round(this.canvas.clientWidth * ratio)), height = Math.max(1, Math.round(this.canvas.clientHeight * ratio));
        if (!this.gpuState.targets.fit(width, height, Math.max(this.scaleNow, MAX_SCALE))) return false;
        this.canvas.width = width;
        this.canvas.height = height;
        return true;
    }

    /** Frame of terrain.wgsl; `pixel` is the metres per pixel at 1 m from the eye, `light` the key light of `lights()`. */
    private uniforms(eye: Eye, viewProj: Float32Array, pixel: number, light: number[]): ArrayBuffer {
        const data = new ArrayBuffer(FRAME_BYTES), floats = new Float32Array(data);
        floats.set(viewProj, 0);
        floats.set(light.slice(0, 4), 16);
        floats.set([eye.z, CURVATURE], 20);
        new Uint32Array(data)[22] = DEBUG_VIEWS.indexOf(this.extras.debug);
        floats[23] = pixel;
        return data;
    }

    /** The look of the regions about the eye under the view's weather. */
    private regionLook(eye: Eye): EyeLook {
        const [lon, lat] = this.frame.lonLat(eye.east, eye.north);
        return eyeLook(this.groundLook.rules.regions, this.groundLook.regionsAt(lon, lat), this.extras.weather);
    }

    /**
     * The look of shade.wgsl: the light keys at the sun's altitude under the weather, with the moon's
     * light at night, and the haze, light and grade of the regions about the eye.
     */
    private look(eye: Eye, near: EyeLook, lights: Lights, aspect: number, jitter: [number, number], scale: number, sample: number): ArrayBuffer {
        const [x, y, z, , bearing] = lights.key, light = lightAt(lights.sunAltitude);
        const h = this.pose.heading * RAD, p = this.pose.pitch * RAD, ty = Math.tan(VFOV * RAD / 2), tx = ty * aspect;
        const forward = [Math.sin(h) * Math.cos(p), Math.cos(h) * Math.cos(p), Math.sin(p)], right = [Math.cos(h), -Math.sin(h), 0];
        const up = [right[1] * forward[2] - right[2] * forward[1], right[2] * forward[0] - right[0] * forward[2], right[0] * forward[1] - right[1] * forward[0]];
        return lookUniform({
            sun: [x, y, z, bearing], skySun: lights.sun, light: weatherLight(lights.share > 0 ? moonlit(light, lights.share, lights.moonAir) : light, near.weather),
            eye: near, altitude: eye.z, place: [eye.east, eye.north],
            camera: { right: right.map(v => v * tx), up: up.map(v => v * ty), forward },
            codes: this.groundLook.codes,
            debug: DEBUG_VIEWS.indexOf(this.extras.debug), sample,
            jitter, scale,
        });
    }

    /**
     * The sun and the moon at the view's place, date and time, in the frame. `key` is the light that
     * shades and shadows: the direction to it, its light, and its bearing in turns from true north. The
     * moon takes the sun's place once the sun's light is gone.
     */
    private lights(): Lights {
        const { east, north } = this.pose, place = this.frame.lonLat(east, north);
        const [hours, minutes] = this.extras.time.split(':').map(Number), minute = 60 * hours + minutes;
        let instant: number;
        try {
            instant = instantAt(this.extras.date, minute, TIME_ZONE);
        } catch {
            // The clock skips this time when it changes to summer time.
            instant = instantAt(this.extras.date, minute + 60, TIME_ZONE);
        }
        // East, north and up to the frame, whose north turns from true north by the convergence.
        const c = this.convergence(east, north) * RAD, cos = Math.cos(c), sin = Math.sin(c);
        const toFrame = ([e, n, u]: number[]) => [e * cos + n * sin, n * cos - e * sin, u];
        const { bearing, altitude } = solarPosition(place, solarTerms(instant));
        const sun = toFrame([Math.sin(bearing) * Math.cos(altitude), Math.cos(bearing) * Math.cos(altitude), Math.sin(altitude)]);
        const toMoon = moonAt(place, instant), moon = toFrame(toMoon);
        const sunAltitude = altitude / RAD, moonAltitude = Math.asin(toMoon[2]) / RAD;
        const phase = Math.acos(Math.min(1, Math.max(-1, -(sun[0] * moon[0] + sun[1] * moon[1] + sun[2] * moon[2])))) / RAD;
        const share = moonShare(sunAltitude, moonAltitude, phase), weather = WEATHERS[this.extras.weather].sun;
        const turns = (b: number) => ((b / (2 * Math.PI)) % 1 + 1) % 1;
        const key = share > 0
            ? [...moon, weather * share, turns(Math.atan2(toMoon[0], toMoon[1]))]
            : [...sun, weather * Math.min(1, Math.max(0, (altitude + 0.05) / 0.15)), turns(bearing)];
        const m = starFrame(place, instant);
        const stars = [0, 1, 2].map(j => cos * m[j] + sin * m[3 + j]).concat([0, 1, 2].map(j => cos * m[3 + j] - sin * m[j]), m.slice(6)) as Mat3;
        return { key, sun, sunAltitude, moon, moonAltitude, moonAir: airColour(moonAltitude), phase, share, stars };
    }
}

/** The sun and the moon of `lights()`: directions in the frame, altitudes and the phase angle in degrees. */
interface Lights {
    key: number[];
    sun: number[];
    sunAltitude: number;
    moon: number[];
    moonAltitude: number;
    /** The air's colour at the moon's altitude (`airColour`). */
    moonAir: [number, number, number];
    phase: number;
    /** The moon's share of its full light, 0 while the sun is the light. */
    share: number;
    /** J2000 equatorial directions to the frame, rows first. */
    stars: Mat3;
}

/** A view projection whose image moves by `ndc` in clip space. */
function jittered(m: Float32Array, [x, y]: [number, number]): Float32Array {
    const out = m.slice();
    for (let j = 0; j < 4; j++) {
        out[4 * j] += x * m[4 * j + 3];
        out[4 * j + 1] += y * m[4 * j + 3];
    }
    return out;
}

/** The clear colour of a `#rrggbb` horizon. */
const horizon = (hex: string): GPUColor => {
    const [r, g, b] = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255);
    return { r, g, b, a: 1 };
};
