// The measuring script of phone.mjs. On /phone/ it draws the start and the end page; in the TrailView page and the timing
// page it measures, but only while a session of this tab runs. A session is a list of runs, each a fresh load of one
// variant at one scene: the times from the navigation start to the first frame and to the first proper render, then a
// flight of FLIGHT_S seconds whose pose follows the clock. Every stage posts what it has to /result, so a tab that the
// system kills leaves its data. A classic script, injected before the page's module: it wraps the WebGPU calls first.
(() => {
    const KEY = 'obc-phone';
    const FLIGHT_S = 20;
    /** From the navigation start to the first proper render, else the run fails. */
    const READY_MS = 90_000;
    /** A frame that takes longer stops the flight. */
    const FRAME_MS = 10_000;
    /** Every Nth frame of a flight times its submit to onSubmittedWorkDone, and samples the memory and the look. */
    const EVERY = 10;
    const TIMING_MS = 180_000;
    const VARIANTS = [
        { id: 'today', name: "today's app", query: '' },
        { id: 'new', name: 'new roads', query: '&ground=plan' },
        // Half the render pixels of the default 0.7 per axis, and half the terrain page pools. The finest level stays, so the meshes can draw.
        { id: 'new-low', name: 'new roads, lighter', query: '&ground=plan&scale=0.5&pool=0.5' },
    ];
    const TAIL = '&date=2026-08-15&time=15:00&weather=sunny';
    // The first three are the walk, valley-drive and zoom-out of flights.json; the last drives over the viaduct into the motorway tunnel north of Stans.
    const SCENES = [
        { id: 'walk', name: 'village walk', from: `?lat=46.820500&lon=8.403000&heading=160.0&pitch=0.0&eye=1.6${TAIL}`, to: { lat: 46.817, lon: 8.405, heading: 200, pitch: 5, height: 1.6 } },
        { id: 'valley-drive', name: 'valley drive', from: `?lat=46.818000&lon=8.380000&heading=80.0&pitch=0.0&eye=3.0${TAIL}`, to: { lat: 46.821, lon: 8.445, heading: 100, pitch: 0, height: 3 } },
        { id: 'zoom-out', name: 'rising above the valley', from: `?lat=46.790000&lon=8.415000&heading=0.0&pitch=-20.0&alt=1500.0${TAIL}`, to: { lat: 46.56, lon: 8.415, heading: 0, pitch: -66, height: 60000 } },
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
    const hooks = { device: undefined, firstCanvas: undefined, lastSubmit: 0 };
    function hook() {
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
        const submit = GPUQueue.prototype.submit;
        GPUQueue.prototype.submit = function (buffers) { hooks.lastSubmit = performance.now(); return submit.call(this, buffers); };
        const current = GPUCanvasContext.prototype.getCurrentTexture;
        GPUCanvasContext.prototype.getCurrentTexture = function () { hooks.firstCanvas ??= performance.now(); return current.call(this); };
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

    async function measure() {
        const i = state.index, { variant, scene } = state.plan[i], key = `runs.${i}`;
        const label = `Run ${i + 1} of ${state.plan.length}: ${VARIANTS[variant].name}, ${SCENES[scene].name}`;
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
        const marks = {};
        const mark = name => () => (marks[name] ??= round(performance.now()));
        tv.eyeReady().then(mark('eyeReady'), e => event(`eyeReady: ${e.message}`));
        tv.nearReady().then(mark('nearReady'), e => event(`nearReady: ${e.message}`));
        // The first proper render: the near field is final (nearReady), and no road mesh, feature tile, record block or piece mesh
        // is on its way (meshesLoading: with ground=plan, every wanted cell has its block and every mesh cell draws).
        const proper = () => marks.nearReady !== undefined && !tv.terrain.meshesLoading;
        while (performance.now() < deadline && marks.proper === undefined) {
            const now = await frame();
            if (marks.firstFrame === undefined && tv.stats().drawn > 0) marks.firstFrame = round(now);
            if (proper()) marks.proper = round(now);
        }
        marks.firstCanvas = hooks.firstCanvas && round(hooks.firstCanvas);
        const canvas = document.querySelector('canvas');
        Object.assign(run, { stage: 'ready', marks, canvas: [canvas.width, canvas.height], memory: { poolsReady: poolBytes(tv) } });
        if (marks.proper === undefined) {
            await post(key, { ...run, stage: 'failed', failure: `no proper render in ${READY_MS / 1000} s`, ...final(tv, run.memory) });
            return next();
        }
        await post(key, run);
        run.flight = await fly(tv, SCENES[scene], label, key, run.memory);
        Object.assign(run, { stage: 'done' }, final(tv, run.memory));
        await post(key, run);
        next();
    }

    /** What the HUD reports as problems, with the memory and the plan's state at the end. */
    function final(tv, memory) {
        const s = tv.stats(), plan = s.plan;
        Object.assign(memory, { poolsEnd: poolBytes(tv), pools: Object.fromEntries(s.pools), planHeap: s.terrain.records?.heap, jsHeap: performance.memory?.usedJSHeapSize });
        return {
            adapter: s.adapter, scale: s.scale, memory,
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
        const t0 = performance.now();
        for (;;) {
            const t = Math.min(1, (performance.now() - t0) / (FLIGHT_S * 1000)), mix = (a, b) => a + (b - a) * t;
            const height = start.mode === 'fly' ? start.height * (end.height / start.height) ** t : mix(start.height, end.height);
            tv.view = { ...start, lat: mix(start.lat, end.lat), lon: mix(start.lon, end.lon), heading: mix(start.heading, end.heading), pitch: mix(start.pitch, end.pitch), height };
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
            say(`${label} … ${Math.round(t * 100)} %`);
            if (performance.now() - posted > 5000) {
                posted = performance.now();
                post(key, { stage: `flight ${Math.round(t * 100)} %`, flight: { seconds: FLIGHT_S, intervals, cpu, look } });
            }
            if (t >= 1) break;
        }
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
        };
        return { seconds: FLIGHT_S, summary, look, intervals, cpu, done };
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
        document.body.replaceChildren(big('TrailView test on this phone. It takes about 8 minutes.'), big('Tap Start, put the phone on the table and do not touch it until it says "Done".'), button);
        button.onclick = async () => {
            button.disabled = true;
            state = { session: `${new Date().toISOString().slice(0, 19).replace(/[-:T]/g, '')}-${Math.random().toString(36).slice(2, 6)}`, index: -1, plan: [], results: {} };
            for (const scene of SCENES.keys()) for (const variant of VARIANTS.keys()) state.plan.push({ variant, scene });
            save();
            const wake = await wakeLock();
            await post('device', { ...(await facts()), wakeLock: wake, plan: state.plan.map(p => `${VARIANTS[p.variant].id} ${SCENES[p.scene].id}`), flightSeconds: FLIGHT_S });
            next();
        };
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
    (location.pathname.endsWith('/plan-timing.html') ? timing() : measure()).catch(e => {
        event(`the measuring script failed: ${e.message}`);
        next();
    });
})();
