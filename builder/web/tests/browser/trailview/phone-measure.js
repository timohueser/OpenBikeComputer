// The measuring script of phone.mjs. On /phone/ it draws the start and the end page; in the TrailView page and the timing
// page it measures, but only while a session of this tab runs. A session is a list of runs, each a fresh load of one
// variant at one scene: the times from the navigation start to the first frame and to the first proper render, then a
// flight of FLIGHT_S seconds whose pose follows the clock. Every stage posts what it has to /result, so a tab that the
// system kills leaves its data. A classic script, injected before the page's module: it wraps the WebGPU calls first.
(() => {
    const KEY = 'obc-phone';
    const FLIGHT_S = 20;
    /** Prototype #3656: a run's own flight length in seconds (`soak=` of its variant), over the session's. */
    const soak = () => Number(new URLSearchParams(location.search).get('soak')) || undefined;
    /** From the navigation start to the first proper render, else the run fails. */
    const READY_MS = (Number(new URLSearchParams(location.search).get('ready')) || 90) * 1000;
    /** A frame that takes longer stops the flight. */
    const FRAME_MS = 10_000;
    /** Every Nth frame of a flight times its submit to onSubmittedWorkDone, and samples the memory and the look. */
    const EVERY = 10;
    const TIMING_MS = 180_000;
    const VARIANTS = [
        { id: 'today', name: "today's app", query: '' },
        // Prototype #3765: today's app at the lighter settings, nearer to the phone display.
        { id: 'today-low', name: "today's app, lighter", query: '&scale=0.5&pool=0.5' },
        ...levers(),
        { id: 'new', name: 'new roads', query: '&ground=plan' },
        // Half the render pixels of the default 0.7 per axis, and half the terrain page pools. The finest level stays, so the meshes can draw.
        { id: 'new-low', name: 'new roads, lighter', query: '&ground=plan&scale=0.5&pool=0.5' },
    ];
    /**
     * Prototype #3765: the phone preset P, and P with one lever changed. Each runs the headroom probe after its flight, but the
     * ballast run, whose 400 MiB filled before the load test whether the load's peak stays under about 1 GB.
     */
    function levers() {
        const P = { scale: 0.5, pool: 0.5, gpool: 0.33, shadows: 1, trees: 0.5, objects: 0, loads: 0.5, fps: 30 };
        const v = (id, name, change, probe = { headroom: 1 }) => ({ id, name, query: `&${new URLSearchParams({ ...P, ...change, ...probe })}` });
        return [
            v('p', 'phone preset', {}), v('p-shadows2', 'P, far shadows on', { shadows: 2 }), v('p-shadows0', 'P, no shadows', { shadows: 0 }),
            v('p-trees1', 'P, full tree range', { trees: 1 }), v('p-gpool1', 'P, full ground pool', { gpool: 1 }), v('p-pool1', 'P, full terrain pool', { pool: 1 }),
            v('p-objects1', 'P, small objects on', { objects: 1 }), v('p-loads1', 'P, all loads at once', { loads: 1 }), v('p-scale07', 'P, scale 0.7', { scale: 0.7 }),
            v('p-terrain08', 'P, coarser terrain', { terrain: 0.8 }), v('p-finest17', 'P, finest level 17', { finest: 17 }), v('p-fps60', 'P at 60 fps', { fps: 60 }),
            v('p-ballast', 'P with 400 MiB ballast', {}, { ballast: 400 }),
            v('p-mac', 'P, no probe', {}, { ready: 300 }),
            v('p-empty', 'P with no data, 30 s', {}, { headroom: 1, empty: 1 }),
            // Prototype #3656: what the probe gains when the view stands still for 60 s, when the workers end, and when the engine ends.
            v('p-wait', 'P, then 60 s still', {}, { headroom: 1, after: 'wait' }),
            v('p-workers', 'P, then the workers end', {}, { headroom: 1, after: 'workers' }),
            v('p-destroy', 'P, then the engine ends', {}, { headroom: 1, after: 'destroy' }),
            v('p-empty-workers', 'P with no data, then the workers end', {}, { headroom: 1, empty: 1, after: 'workers' }),
            v('p-end-ground', 'P, then the ground workers end', {}, { headroom: 1, after: 'end:ground-worker' }),
            v('p-end-stamp', 'P, then the stamp worker ends', {}, { headroom: 1, after: 'end:stamp-worker' }),
            v('p-end-road', 'P, then the road worker ends', {}, { headroom: 1, after: 'end:road-worker' }),
            v('p-soak', 'P, the path to and fro for 10 minutes', {}, { headroom: 1, soak: 600 }),
            v('p-end-rest', 'P, then the small workers end', {}, { headroom: 1, after: 'end:decode-worker|model-worker|house-worker|plant-worker' }),
            // Prototype #3656: the turn prototype's `reuse=1`, and the same at 10 scene frames a second with a narrower margin.
            v('p-reuse', 'P, turn reuse', { reuse: 1 }),
            v('p-reuse10', 'P, turn reuse at 10/s, margin 1.15', { reuse: 1, turn: 10, margin: 1.15 }),
        ];
    }
    const TAIL = '&date=2026-08-15&time=15:00&weather=sunny';
    // The first three are the walk, valley-drive and zoom-out of flights.json; the last drives over the viaduct into the motorway tunnel north of Stans.
    const WALK = `?lat=46.820500&lon=8.403000&heading=160.0&pitch=0.0&eye=1.6${TAIL}`;
    /** Prototype #3656: 30° right, back, 30° left, back, at 45° a second, twice: the heading offset at `s` seconds. */
    const LOOK = [0, 30, 0, -30, 0, 30, 0, -30, 0], LEG_S = 30 / 45;
    const look = s => { const i = Math.min(LOOK.length - 2, Math.floor(s / LEG_S)), f = Math.min(1, s / LEG_S - i); return LOOK[i] + (LOOK[i + 1] - LOOK[i]) * f; };
    const SCENES = [
        { id: 'walk', name: 'village walk', from: WALK, to: { lat: 46.817, lon: 8.405, heading: 200, pitch: 5, height: 1.6 } },
        { id: 'valley-drive', name: 'valley drive', from: `?lat=46.818000&lon=8.380000&heading=80.0&pitch=0.0&eye=3.0${TAIL}`, to: { lat: 46.821, lon: 8.445, heading: 100, pitch: 0, height: 3 } },
        { id: 'zoom-out', name: 'rising above the valley', from: `?lat=46.790000&lon=8.415000&heading=0.0&pitch=-20.0&alt=1500.0${TAIL}`, to: { lat: 46.56, lon: 8.415, heading: 0, pitch: -66, height: 60000 } },
        // Prototype #3765: a place without data, for the engine's fixed memory.
        { id: 'nowhere', name: 'no data', from: `?lat=45.000000&lon=2.000000&heading=0.0&pitch=0.0&eye=1.6${TAIL}`, to: { lat: 45, lon: 2, heading: 0, pitch: 0, height: 1.6 } },
        // Prototype #3656: the eye stands at the start of the walk. It turns once round in 8 s, looks about, or only loads for 15 s.
        { id: 'turn', name: 'full turn', from: WALK, turn: s => 360 * s / 8, seconds: 8 },
        { id: 'look', name: 'look about', from: WALK, turn: look, seconds: LOOK.length * LEG_S - LEG_S, noProbe: true },
        { id: 'load', name: 'cold load', from: WALK, load: 15 },
        { id: 'bridge-tunnel', name: 'bridge and tunnel', from: `?lat=46.9631796&lon=8.5085956&heading=83.0&pitch=-2.0&alt=487.0${TAIL}`, to: { lat: 46.96361, lon: 8.5137562, heading: 83, pitch: -2, height: 487 } },
    ];

    const load = () => { try { return JSON.parse(sessionStorage.getItem(KEY)) ?? undefined; } catch { return undefined; } };
    const save = () => { try { sessionStorage.setItem(KEY, JSON.stringify(state)); } catch { /* the next load starts the session again */ } };
    let state = load();
    const round = value => Math.round(value * 100) / 100;
    const post = (key, value) => {
        remember(key, value);
        const body = JSON.stringify({ session: state.session, key, value });
        return fetch('/result', { method: 'POST', headers: { 'content-type': 'application/json' }, body, keepalive: body.length < 60_000 }).catch(() => {});
    };
    let errors = 0;
    const event = text => {
        if (!state || errors++ > 40) return;
        post('event', { run: state.index, text: String(text).slice(0, 500) });
    };

    // The page asks for a WebGPU adapter and device itself: these wrappers report each failure and keep the device.
    const hooks = { device: undefined, firstCanvas: undefined, canvas: [], frames: undefined, lastSubmit: 0, flightStart: Infinity, ground: { open: undefined, jobs: [] }, workers: [] };
    function hook() {
        const ballast = Number(new URLSearchParams(location.search).get('ballast')) || 0;
        if (ballast) window.obcBallast = Array.from({ length: ballast / 16 }, () => random(new Uint32Array(4 * 2 ** 20)));
        const Base = window.Worker;
        window.Worker = class extends Base {
            constructor(url, options) {
                super(url, options);
                hooks.workers.push([this, String(url)]);
                if (String(url).includes('ground-worker')) tapGround(this, hooks.ground);
            }
        };
        addEventListener('error', e => event(`page error: ${e.message}`));
        addEventListener('unhandledrejection', e => event(`unhandled rejection: ${e.reason?.message ?? e.reason}`));
        const error = console.error;
        console.error = (...args) => { event(`console.error: ${args.map(String).join(' ')}`); error.apply(console, args); };
        if (!navigator.gpu) return event('the browser has no navigator.gpu');
        const requestAdapter = GPU.prototype.requestAdapter;
        GPU.prototype.requestAdapter = async function (...args) {
            try {
                const adapter = await requestAdapter.apply(this, args);
                if (!adapter) event('the browser gave no WebGPU adapter');
                return adapter;
            } catch (e) { event(`the WebGPU adapter request failed: ${e.message}`); throw e; }
        };
        const requestDevice = GPUAdapter.prototype.requestDevice;
        GPUAdapter.prototype.requestDevice = async function (...args) {
            try {
                const device = hooks.device = await requestDevice.apply(this, args);
                device.addEventListener('uncapturederror', e => event(`WebGPU error: ${e.error.message}`));
                device.lost.then(info => info.reason !== 'destroyed' && event(`the WebGPU device was lost (${info.reason}): ${info.message}`));
                return device;
            } catch (e) { event(`the WebGPU device request failed: ${e.message}`); throw e; }
        };
        // Prototype #3765: the bytes of the GPU objects that the page creates, by kind, and its shaders and pipelines.
        const BPP = { r8unorm: 1, rg8unorm: 2, r16float: 2, depth16unorm: 2, rgba16float: 8, rgba16uint: 8, rg32float: 8, rgba32float: 16 };
        const bytesOf = d => {
            const [w, h = 1, l = 1] = Array.isArray(d.size) ? d.size : [d.size.width, d.size.height ?? 1, d.size.depthOrArrayLayers ?? 1];
            const per = BPP[d.format] ?? (/bc|etc|astc/.test(d.format) ? 1 : 4), mips = (d.mipLevelCount ?? 1) > 1 ? 4 / 3 : 1;
            return w * h * l * per * mips * (d.sampleCount ?? 1);
        };
        const alloc = hooks.alloc = { buffers: 0, textures: 0, bufferCount: 0, textureCount: 0, shaderBytes: 0, shaders: 0, pipelines: 0, largestTextures: [] };
        const track = (object, bytes, kind) => {
            alloc[kind] += bytes;
            alloc[kind === 'buffers' ? 'bufferCount' : 'textureCount']++;
            const destroy = object.destroy.bind(object);
            object.destroy = () => { alloc[kind] -= bytes; alloc[kind === 'buffers' ? 'bufferCount' : 'textureCount']--; bytes = 0; return destroy(); };
            return object;
        };
        const createBuffer = GPUDevice.prototype.createBuffer;
        GPUDevice.prototype.createBuffer = function (d) { return track(createBuffer.call(this, d), d.size, 'buffers'); };
        const createTexture = GPUDevice.prototype.createTexture;
        GPUDevice.prototype.createTexture = function (d) {
            const bytes = bytesOf(d);
            if (bytes > 8 * 2 ** 20) alloc.largestTextures.push({ label: d.label ?? '', format: d.format, size: d.size, mib: Math.round(bytes / 2 ** 20) });
            return track(createTexture.call(this, d), bytes, 'textures');
        };
        const createShaderModule = GPUDevice.prototype.createShaderModule;
        GPUDevice.prototype.createShaderModule = function (d) { alloc.shaders++; alloc.shaderBytes += d.code?.length ?? 0; return createShaderModule.call(this, d); };
        for (const name of ['createRenderPipeline', 'createComputePipeline', 'createRenderPipelineAsync', 'createComputePipelineAsync']) {
            const base = GPUDevice.prototype[name];
            GPUDevice.prototype[name] = function (...args) { alloc.pipelines++; return base.apply(this, args); };
        }
        const submit = GPUQueue.prototype.submit;
        GPUQueue.prototype.submit = function (buffers) { hooks.lastSubmit = performance.now(); return submit.call(this, buffers); };
        const current = GPUCanvasContext.prototype.getCurrentTexture;
        GPUCanvasContext.prototype.getCurrentTexture = function () { hooks.firstCanvas ??= performance.now(); hooks.canvas.push(performance.now()); return current.call(this); };
    }

    let seed = 0x9e3779b9;
    /** Random words: iOS compresses memory, and zeros would cost it almost nothing. */
    function random(words) {
        for (let i = 0; i < words.length; i++) { seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5; words[i] = seed; }
        return words;
    }
    /** Prototype #3656: what happens between the view and the probe (`after=`): 60 s still, the workers end, or the engine ends. */
    async function after(tv, key, label) {
        const what = new URLSearchParams(location.search).get('after');
        if (!what) return;
        // `end:a|b` ends only the workers whose script name holds a or b.
        const names = what.startsWith('end:') ? what.slice(4).split('|') : what === 'wait' ? [] : [''];
        const ending = hooks.workers.filter(([, url]) => names.some(name => url.includes(name)));
        if (what === 'destroy') tv.destroy();
        for (const [worker] of ending) worker.terminate();
        const ms = what === 'wait' ? 60_000 : 10_000;
        say(`${label} … ${what}, then ${ms / 1000} s`);
        await sleep(ms);
        await post(key, { after: what, afterMs: ms, workersEnded: ending.map(([, url]) => url.split('/').pop()) });
    }
    /** Fills memory in steps of 32 MiB until the system stops the tab; the last post is the headroom over the view. */
    async function headroom(key) {
        const keep = [];
        for (let mib = 32; mib <= 3072; mib += 32) {
            keep.push(random(new Uint32Array(8 * 2 ** 20)));
            await post(key, { headroomMiB: mib });
        }
    }
    const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
    const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
    async function until(test, deadline) {
        for (;;) {
            const value = test();
            if (value || performance.now() > deadline) return value;
            await sleep(50);
        }
    }
    const ready = new Promise(resolve => (document.readyState === 'loading' ? addEventListener('DOMContentLoaded', resolve) : resolve()));
    let banner;
    async function say(text) {
        await ready;
        if (!banner) {
            banner = document.createElement('div');
            banner.style.cssText = 'position:fixed;left:0;right:0;top:0;z-index:99999;padding:16px;background:rgb(0 0 0 / .7);color:#fff;font:600 22px/1.3 system-ui,sans-serif;pointer-events:none';
            document.head.append(Object.assign(document.createElement('style'), { textContent: '#hud{display:none!important}' }));
            document.body.append(banner);
        }
        banner.textContent = text;
    }
    async function wakeLock() {
        if (!('wakeLock' in navigator)) return 'none';
        try { await navigator.wakeLock.request('screen'); return 'on'; } catch (e) { return `refused: ${e.message}`; }
    }
    const url = i => {
        const { variant, scene } = state.plan[i];
        return `/trailview.html${SCENES[scene].from}${VARIANTS[variant].query}`;
    };
    /** Each run starts cold: the answers are no-store, and the engine's Cache Storage goes. */
    async function next() {
        state.index++;
        save();
        try { for (const name of await caches.keys()) await caches.delete(name); } catch { /* no Cache Storage */ }
        if (state.index < state.plan.length) return location.replace(url(state.index));
        const timing = await fetch('/phone/timing.json').then(r => r.json()).catch(() => ({}));
        if (timing.url && !state.timed) {
            state.timed = true;
            save();
            return location.replace(timing.url);
        }
        state.finished = true;
        save();
        location.replace('/phone/');
    }

    const quantile = (list, q) => { const s = [...list].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(q * s.length))] : undefined; };
    const share = (list, ms) => (list.length ? (100 * list.filter(v => v > ms).length) / list.length : undefined);
    const poolBytes = tv => [...(tv.gpuState?.gpu.pools.values() ?? [])].reduce((a, b) => a + b, 0);
    const range = (r, v) => (v === undefined ? r : r ? [Math.min(r[0], v), Math.max(r[1], v)] : [v, v]);

    /** Times the jobs of a ground worker: the first message is the decode probe that GroundSource.open sends. */
    function tapGround(worker, ground) {
        const sent = new Map(), post = worker.postMessage.bind(worker);
        worker.postMessage = (data, ...rest) => {
            const now = performance.now();
            ground.open ??= now;
            if (data?.job) sent.set(data.id, now);
            return post(data, ...rest);
        };
        worker.addEventListener('message', ({ data }) => {
            const start = sent.get(data?.id);
            if (start === undefined) return;
            sent.delete(data.id);
            ground.jobs.push({ start, ms: performance.now() - start, failed: Boolean(data.error) });
        });
    }

    /** The ground jobs sent from `from` to `until`: their count, round trips in ms, and the ms from the probe to the last of their pages. */
    function groundSummary(ground, from, until) {
        const jobs = ground.jobs.filter(job => job.start >= from && job.start < until), ms = jobs.map(job => job.ms);
        if (!jobs.length) return { jobs: 0 };
        return {
            jobs: jobs.length, failed: jobs.filter(job => job.failed).length, median: round(quantile(ms, 0.5)), p95: round(quantile(ms, 0.95)),
            sum: round(ms.reduce((a, b) => a + b, 0)), openToLast: round(Math.max(...jobs.map(job => job.start + job.ms)) - ground.open),
        };
    }

    async function measure() {
        const i = state.index, { variant, scene } = state.plan[i], key = `runs.${i}`;
        const label = `Run ${i + 1} of ${state.plan.length}: ${VARIANTS[variant].name}, ${SCENES[scene].name}`;
        if (state.loaded === i && state.probing === i) {
            await post(key, { headroomEnd: 'the system stopped the tab' });
            return next();
        }
        if (state.loaded === i) {
            event('the system reloaded the tab during this run');
            await post(key, { failure: 'the system reloaded the tab during this run' });
            return next();
        }
        state.loaded = i;
        save();
        say(`${label} … getting ready`);
        const run = { variant: VARIANTS[variant].id, scene: SCENES[scene].id, url: location.href, started: new Date().toISOString(), cache: 'cold: no-store answers, Cache Storage cleared', wakeLock: await wakeLock(), stage: 'loading' };
        post(key, run);
        const deadline = READY_MS;
        const notice = () => document.querySelector('.notice')?.textContent;
        const tv = await until(() => window.trailView ?? (notice() && 'notice'), deadline);
        if (tv === 'notice' || !tv) {
            await post(key, { stage: 'failed', failure: tv ? `the page says: ${notice()}` : `no viewer after ${READY_MS / 1000} s` });
            return next();
        }
        if (new URLSearchParams(location.search).get('empty')) {
            say(`${label} … waiting 30 s`);
            await sleep(30_000);
            await post(key, { stage: 'empty', memory: { poolsEnd: poolBytes(tv), alloc: hooks.alloc && { ...hooks.alloc, buffers: Math.round(hooks.alloc.buffers / 2 ** 20), textures: Math.round(hooks.alloc.textures / 2 ** 20) } } });
            await after(tv, key, label);
            state.probing = i;
            save();
            say(`${label} … memory headroom`);
            await headroom(key);
            return next();
        }
        if (SCENES[scene].load) return loadOnly(tv, run, key, label, SCENES[scene].load * 1000);
        const marks = {};
        const mark = name => () => (marks[name] ??= round(performance.now()));
        tv.eyeReady().then(mark('eyeReady'), e => event(`eyeReady: ${e.message}`));
        tv.nearReady().then(mark('nearReady'), e => event(`nearReady: ${e.message}`));
        // The first proper render: the near field is final (nearReady), and no road mesh, feature tile, record block or piece mesh
        // is on its way (meshesLoading: with ground=plan, every wanted cell has its block and every mesh cell draws).
        const proper = () => marks.nearReady !== undefined && !tv.terrain.meshesLoading;
        const series = [];
        let sampled = 0;
        while (performance.now() < deadline && marks.proper === undefined) {
            const now = await frame();
            if (SCENES[scene].turn && !hooks.frames && tv.gpuState?.times) tapTimes(tv.gpuState.times);
            if (hooks.alloc && now - sampled > 1000) {
                sampled = now;
                series.push([Math.round(now), Math.round(hooks.alloc.buffers / 2 ** 20), Math.round(hooks.alloc.textures / 2 ** 20), hooks.alloc.pipelines]);
            }
            if (marks.firstFrame === undefined && tv.stats().drawn > 0) marks.firstFrame = round(now);
            if (proper()) marks.proper = round(now);
        }
        marks.firstCanvas = hooks.firstCanvas && round(hooks.firstCanvas);
        const canvas = document.querySelector('canvas');
        Object.assign(run, { allocSeries: series, wallStart: Date.now() - performance.now() });
        Object.assign(run, { stage: 'ready', marks, canvas: [canvas.width, canvas.height], memory: { poolsReady: poolBytes(tv) } });
        await userAgentMemory().catch(e => event(`memory: ${e.message}`));
        if (marks.proper === undefined) {
            await post(key, { ...run, stage: 'failed', failure: `no proper render in ${READY_MS / 1000} s`, ...final(tv, run.memory) });
            return next();
        }
        if (SCENES[scene].turn) {
            say(`${label} … waiting until the view is still`);
            run.idle = await Promise.race([tv.waitForIdle().then(() => round(performance.now())), sleep(30_000).then(() => 'not in 30 s')]);
        }
        await post(key, run);
        hooks.flightStart = performance.now();
        run.flight = await fly(tv, SCENES[scene], label, key, run.memory);
        Object.assign(run, { stage: 'done' }, final(tv, run.memory));
        await post(key, run);
        if (new URLSearchParams(location.search).get('headroom') && !SCENES[scene].noProbe) {
            await after(tv, key, label);
            state.probing = i;
            save();
            say(`${label} … memory headroom`);
            await headroom(key);
        }
        next();
    }

    /** Chromium's memory split by worker, where the page is cross-origin isolated. */
    async function userAgentMemory() {
        if (!performance.measureUserAgentSpecificMemory || !crossOriginIsolated) return;
        const m = await performance.measureUserAgentSpecificMemory();
        window.obcUserAgentMemory = { totalMiB: Math.round(m.bytes / 2 ** 20), parts: m.breakdown.filter(b => b.bytes > 2 ** 20).map(b => ({ mib: Math.round(b.bytes / 2 ** 20), types: b.types, where: b.attribution.map(a => (a.scope ?? '') + ' ' + String(a.url).split('/').pop().slice(0, 40)) })) };
    }
    /** What the HUD reports as problems, with the memory and the plan's state at the end. */
    function final(tv, memory) {
        const s = tv.stats(), plan = s.plan;
        if (hooks.alloc) memory.alloc = { ...hooks.alloc, buffers: Math.round(hooks.alloc.buffers / 2 ** 20), textures: Math.round(hooks.alloc.textures / 2 ** 20), shaderKiB: Math.round(hooks.alloc.shaderBytes / 1024) };
        memory.userAgent = window.obcUserAgentMemory;
        Object.assign(memory, { poolsEnd: poolBytes(tv), pools: Object.fromEntries(s.pools), planHeap: s.terrain.records?.heap, jsHeap: performance.memory?.usedJSHeapSize });
        return {
            adapter: s.adapter, scale: s.scale, memory,
            // The load's jobs (sent before the flight) and the flight's.
            ground: { load: groundSummary(hooks.ground, 0, hooks.flightStart), flight: groundSummary(hooks.ground, hooks.flightStart, Infinity) },
            problems: {
                recoveries: s.recoveries, failed: s.failed, groundShort: s.ground.shortage, instancesDropped: s.instances.dropped, stampLimits: s.terrain.stampLimits,
                quality: s.terrain.quality, left: s.terrain.left, records: s.terrain.records?.error, recordsFailed: s.terrain.records?.failed, recordRestarts: s.terrain.records?.restarts,
                plan: plan?.error, waterDropped: s.terrain.waterDropped, structuresDropped: s.terrain.structuresDropped,
            },
            records: s.terrain.records, pieces: s.terrain.pieces,
        };
    }

    /** The flight: the pose follows the clock, one pose per drawn frame, as flight.mjs moves it per frame. */
    async function fly(tv, scene, label, key, memory) {
        const start = { ...tv.view }, end = { ...start, ...scene.to };
        const intervals = [], cpu = [], done = [], gpu = [], passes = {}, look = {};
        let last, lastTimes, n = 0, posted = performance.now();
        const t0 = performance.now(), base = scene.seconds ?? FLIGHT_S;
        for (;;) {
            // A soak (state.seconds over FLIGHT_S) flies the path to and fro until its time is over.
            const elapsed = (performance.now() - t0) / (base * 1000), seconds = scene.seconds ?? soak() ?? state.seconds ?? FLIGHT_S, over = elapsed * base >= seconds;
            const t = seconds > base ? (elapsed % 2 < 1 ? elapsed % 1 : 1 - (elapsed % 1)) : Math.min(1, elapsed), mix = (a, b) => a + (b - a) * t;
            const height = start.mode === 'fly' ? start.height * (end.height / start.height) ** t : mix(start.height, end.height);
            tv.view = scene.turn ? { ...start, heading: (start.heading + scene.turn(t * base) + 360) % 360 }
                : { ...start, lat: mix(start.lat, end.lat), lon: mix(start.lon, end.lon), heading: mix(start.heading, end.heading), pitch: mix(start.pitch, end.pitch), height };
            const drawn = await Promise.race([tv.nextFrame().then(() => true), sleep(FRAME_MS)]);
            if (!drawn) {
                event(`a frame took over ${FRAME_MS / 1000} s; the flight stops`);
                break;
            }
            // The drawn frame ends inside its animation frame callback: the timeline holds that callback's time.
            const now = document.timeline?.currentTime ?? performance.now();
            if (last !== undefined) {
                intervals.push(round(now - last));
                cpu.push(round(tv.cpu));
                if (hooks.frames?.length) hooks.frames.at(-1).cpu = tv.cpu;
            }
            last = now;
            n++;
            const times = tv.gpuState?.times;
            const tag = times && (times.tag ?? JSON.stringify(times.ms));
            if (times && tag !== lastTimes && Object.keys(times.ms).length) {
                lastTimes = tag;
                gpu.push(round(Object.values(times.ms).reduce((a, b) => a + b, 0)));
                for (const [pass, ms] of Object.entries(times.ms)) (passes[pass] ??= []).push(ms);
            }
            if (n % EVERY === 0) {
                const queue = hooks.device?.queue, submitted = hooks.lastSubmit;
                queue?.onSubmittedWorkDone().then(() => done.push(round(performance.now() - submitted)));
                const s = tv.stats(), pieces = s.terrain.pieces;
                memory.poolsPeak = Math.max(memory.poolsPeak ?? 0, poolBytes(tv));
                if (performance.memory) memory.jsHeapPeak = Math.max(memory.jsHeapPeak ?? 0, performance.memory.usedJSHeapSize);
                if (s.terrain.records) memory.planHeapPeak = Math.max(memory.planHeapPeak ?? 0, s.terrain.records.heap);
                look.quality = range(look.quality, s.terrain.quality);
                look.finest = range(look.finest, Math.max(0, ...(tv.terrain.nodes ?? []).map(node => node.level)));
                look.piecesDrawn = range(look.piecesDrawn, pieces?.drawn);
                look.piecesInView = range(look.piecesInView, pieces?.inView);
                look.recordCells = range(look.recordCells, s.terrain.records?.cells);
            }
            const done_ = Math.min(100, Math.round((100 * elapsed * base) / seconds));
            say(`${label} … ${done_} %`);
            if (performance.now() - posted > (seconds > FLIGHT_S ? 30_000 : 5000)) {
                posted = performance.now();
                post(key, { stage: `flight ${done_} %`, flight: { seconds, intervals, cpu, look } });
            }
            if (seconds > base ? over : t >= 1) break;
        }
        const t1 = performance.now();
        // The last submits' readbacks.
        await sleep(300);
        const summary = {
            frames: intervals.length,
            interval: { median: quantile(intervals, 0.5), p95: quantile(intervals, 0.95), p99: quantile(intervals, 0.99), max: Math.max(...intervals) },
            // One display period of 60 Hz and two, with 0.8 ms for the jitter of the frame clock.
            over16: share(intervals, 17.5), over33: share(intervals, 34.2),
            cpu: { median: quantile(cpu, 0.5), p95: quantile(cpu, 0.95) },
            done: { median: quantile(done, 0.5), p95: quantile(done, 0.95), samples: done.length },
            gpu: gpu.length ? { median: quantile(gpu, 0.5), p95: quantile(gpu, 0.95), samples: gpu.length, passes: Object.fromEntries(Object.entries(passes).map(([p, l]) => [p, round(quantile(l, 0.5))])) } : null,
            reuse: hooks.frames && engineFrames(t0, t1),
        };
        return { seconds: soak() ?? state.seconds ?? FLIGHT_S, summary, look, intervals, cpu, done };
    }

    /** Prototype #3656: each engine frame of the turn prototype, with the passes it wrote and, once read back, its GPU time. */
    function tapTimes(times) {
        const frames = (hooks.frames = []), byStart = new Map(), { writes, resolve } = times;
        let passes = [], tag;
        times.writes = function (pass) { passes.push(pass); return writes.call(this, pass); };
        times.resolve = function (encoder, frameTag) {
            const f = { at: frameTag?.frame ?? performance.now(), passes };
            passes = [];
            frames.push(f);
            byStart.set(f.at, f);
            return resolve.call(this, encoder, frameTag);
        };
        Object.defineProperty(times, 'tag', {
            configurable: true, get: () => tag,
            set(value) {
                tag = value;
                const f = byStart.get(value?.frame);
                if (f) Object.assign(f, { gpu: Object.values(times.ms).reduce((a, b) => a + b, 0), present: times.ms.present });
            },
        });
    }
    const mean = list => (list.length ? round(list.reduce((a, b) => a + b, 0) / list.length) : undefined);
    /** Prototype #3656: the engine frames that started in [from, to): scene frames draw the terrain, present-only frames only `present`. */
    function engineFrames(from, to) {
        const list = hooks.frames.filter(f => f.at >= from && f.at < to), stats = l => {
            const gpu = l.filter(f => f.gpu !== undefined).map(f => f.gpu), cpu = l.filter(f => f.cpu !== undefined).map(f => f.cpu);
            return { frames: l.length, timed: gpu.length, gpuMean: mean(gpu), gpuMedian: round(quantile(gpu, 0.5) ?? NaN), gpuSum: round(gpu.reduce((a, b) => a + b, 0)), cpuMean: mean(cpu), cpuMedian: round(quantile(cpu, 0.5) ?? NaN) };
        };
        const kind = f => (f.passes.includes('terrain') ? 'scene' : f.passes.length === 1 && f.passes[0] === 'present' ? 'presentOnly' : 'other');
        const presents = list.filter(f => f.present !== undefined).map(f => f.present);
        return {
            shown: hooks.canvas.filter(t => t >= from && t < to).length, all: stats(list),
            scene: stats(list.filter(f => kind(f) === 'scene')), presentOnly: stats(list.filter(f => kind(f) === 'presentOnly')), other: stats(list.filter(f => kind(f) === 'other')),
            presentPass: { median: round(quantile(presents, 0.5) ?? NaN), samples: presents.length },
        };
    }
    /** Prototype #3656: the first `ms` of a cold load with no input, then the time until the view is idle. */
    async function loadOnly(tv, run, key, label, ms) {
        const idle = tv.waitForIdle().then(() => round(performance.now()), e => `failed: ${e.message}`);
        say(`${label} … loading for ${ms / 1000} s`);
        while (performance.now() < ms) {
            await frame();
            if (!hooks.frames && tv.gpuState?.times) tapTimes(tv.gpuState.times);
        }
        await sleep(300);
        Object.assign(run, { stage: 'loaded', load: engineFrames(0, ms), firstEngineFrame: hooks.frames?.[0] && round(hooks.frames[0].at) });
        await post(key, run);
        say(`${label} … waiting until the view is still`);
        run.idle = await Promise.race([idle, sleep(READY_MS - performance.now()).then(() => `not in ${READY_MS / 1000} s`)]);
        Object.assign(run, { stage: 'done', untilIdle: engineFrames(0, performance.now()) }, final(tv, run.memory ?? {}));
        await post(key, run);
        next();
    }

    async function timing() {
        say('Last step: road building test … please wait');
        const result = await until(() => window.planTiming?.done && window.planTiming, performance.now() + TIMING_MS);
        await post('timing', result ?? { error: `the timing page did not finish in ${TIMING_MS / 1000} s` });
        next();
    }

    /** The start page, and at the end the thank-you with the main numbers of each run, read back from the result file's copy in this tab. */
    async function home() {
        await ready;
        document.body.style.cssText = 'margin:0;padding:24px 16px;font:20px/1.4 system-ui,sans-serif;background:#fff;color:#111';
        const big = text => Object.assign(document.createElement('p'), { textContent: text });
        if (state?.finished) {
            const table = document.createElement('table');
            table.style.cssText = 'border-collapse:collapse;font-size:15px';
            const rows = [['', 'Ready after', 'Smooth', 'Stutters'], ...state.plan.map((p, i) => {
                const r = state.results?.[i];
                return [`${VARIANTS[p.variant].name}, ${SCENES[p.scene].name}`, r?.ready ?? '–', r?.smooth ?? '–', r?.stutters ?? '–'];
            })];
            for (const row of rows) {
                const tr = table.insertRow();
                for (const cell of row) Object.assign(tr.insertCell(), { textContent: cell }).style.cssText = 'padding:4px 6px;border-bottom:1px solid #ccc';
            }
            document.body.replaceChildren(big('Done. Thank you. You can close this page.'), table);
            return;
        }
        const button = Object.assign(document.createElement('button'), { textContent: 'Start' });
        button.style.cssText = 'display:block;width:100%;padding:32px;font:700 32px system-ui;border-radius:16px;border:0;background:#1a7f37;color:#fff';
        document.body.replaceChildren(big(`TrailView test on this phone: ${new URLSearchParams(location.search).get('runs')?.split(',').length ?? SCENES.length * VARIANTS.length} runs.`), big('Tap Start, put the phone on the table and do not touch it until it says "Done".'), button);
        button.onclick = async () => {
            button.disabled = true;
            state = { session: `${new Date().toISOString().slice(0, 19).replace(/[-:T]/g, '')}-${Math.random().toString(36).slice(2, 6)}`, index: -1, plan: [], results: {} };
            // Prototype #3765: ?runs=variant:scene,… picks the runs, ?seconds=N flies each run N seconds, ?host=NAME labels the session.
            const query = new URLSearchParams(location.search), pick = query.get('runs');
            if (pick) for (const run of pick.split(',')) {
                const [v, sc] = run.split(':'), variant = VARIANTS.findIndex(x => x.id === v), scene = SCENES.findIndex(x => x.id === sc);
                if (variant >= 0 && scene >= 0) state.plan.push({ variant, scene });
            }
            else for (const scene of SCENES.keys()) for (const variant of VARIANTS.keys()) state.plan.push({ variant, scene });
            if (query.get('seconds')) state.seconds = Number(query.get('seconds'));
            if (query.get('host')) state.session = `${query.get('host')}-${state.session}`;
            // ?size=WxH fixes the canvas at W×H CSS pixels: a phone's view in a desktop browser.
            if (query.get('size')) state.size = query.get('size').split('x').map(Number);
            save();
            const wake = await wakeLock();
            await post('device', { ...(await facts()), wakeLock: wake, plan: state.plan.map(p => `${VARIANTS[p.variant].id} ${SCENES[p.scene].id}`), flightSeconds: state.seconds ?? FLIGHT_S });
            next();
        };
        // Prototype #3765: ?auto=1 starts at once, for runs that no one taps (the app on a Mac).
        if (new URLSearchParams(location.search).get('auto')) button.click();
    }

    async function facts() {
        const f = { userAgent: navigator.userAgent, screen: [screen.width, screen.height], viewport: [innerWidth, innerHeight], pixelRatio: devicePixelRatio, secure: isSecureContext, cores: navigator.hardwareConcurrency, webgpu: !!navigator.gpu };
        if (!navigator.gpu) return f;
        try {
            const adapter = await navigator.gpu.requestAdapter();
            if (!adapter) return { ...f, adapter: { error: 'no adapter' } };
            const info = adapter.info ?? {}, limits = {};
            for (const name in adapter.limits) limits[name] = adapter.limits[name];
            f.adapter = { info: [info.vendor, info.architecture, info.device, info.description].filter(Boolean).join(' '), features: [...adapter.features].sort(), limits };
            f.format = navigator.gpu.getPreferredCanvasFormat();
        } catch (e) {
            f.adapter = { error: e.message };
        }
        return f;
    }

    /** The main numbers of a run for the thank-you table, kept in the tab: the page cannot read the result file. */
    function remember(key, value) {
        const run = /^runs\.(\d+)$/.exec(key), s = value?.flight?.summary;
        if (!run || !(value.marks || value.failure)) return;
        const r = state.results[run[1]] ??= {};
        if (value.marks?.proper !== undefined) r.ready = `${(value.marks.proper / 1000).toFixed(1)} s`;
        if (value.failure) r.ready = 'did not finish';
        if (s?.over16 !== undefined) Object.assign(r, { smooth: `${Math.round(100 - s.over16)} %`, stutters: `${Math.round(s.over33)} %` });
        save();
    }

    if (location.pathname.startsWith('/phone')) return void home();
    if (!state || state.finished || state.index < 0) return;
    hook();
    if (state.size) document.documentElement.append(Object.assign(document.createElement('style'), { textContent: `canvas{width:${state.size[0]}px!important;height:${state.size[1]}px!important}` }));
    (location.pathname.endsWith('/plan-timing.html') ? timing() : measure()).catch(e => {
        event(`the measuring script failed: ${e.message}`);
        next();
    });
})();
