// The fault check of phone.mjs: /phone/diagnose draws its start page; in the TrailView page it runs one session
// of two loads, today's app and then ground=plan, from the walk view of flights.json. Today's app flies the walk
// flight with a picture every 2 s, then back with debug=normals, and reads the drawn pages after it. Every load
// reports the WebGPU errors per shader module, pipeline and allocation, the console's warnings and the workers'
// errors. For each pipeline that the browser refuses, it finds the function that the compiler rejects: it stubs
// functions of the shader and compiles again. Each finding posts at once, so a tab that the system kills leaves its
// data. A classic script, injected before the page's module: it wraps the WebGPU calls first.
//
// /phone/cold runs the cold start check instead: a cold load of the walk view per debug view of `?views=` (default
// off,normals), with a picture each second from the first drawn frame through the load and the walk flight, the state of
// the engine at each picture, the drawn pages' heights when the load is ready and after the flight, and the time split of
// the ground worker's jobs (phone-worker.js).
(() => {
    const KEY = 'obc-diagnose';
    const TAIL = '&date=2026-08-15&time=15:00&weather=sunny';
    const WALK = { lat: 46.8205, lon: 8.403, heading: 160, pitch: 0, eye: 1.6 };
    const WALK_URL = `/trailview.html?lat=${WALK.lat}&lon=${WALK.lon}&heading=${WALK.heading}&pitch=${WALK.pitch}&eye=${WALK.eye}${TAIL}`;
    // The walk flight of phone-measure.js: 20 s from the walk view to here.
    const WALK_END = { lat: 46.817, lon: 8.405, heading: 200, pitch: 5, height: 1.6 };
    /** Seconds that the refused pipelines may take to bisect, per load. */
    const BISECT_S = 150;
    /** The cold start check: the longest load before the flight starts anyway, and the flight. */
    const COLD_READY_MS = 45_000, FLIGHT_S = 20;

    const load = () => { try { return JSON.parse(sessionStorage.getItem(KEY)) ?? undefined; } catch { return undefined; } };
    const save = () => { try { sessionStorage.setItem(KEY, JSON.stringify(state)); } catch { /* the next load starts again */ } };
    const stepsOf = s => (s?.mode === 'cold'
        ? s.views.map(view => ({ id: `cold-${view}`, url: view === 'off' ? WALK_URL : `${WALK_URL}&debug=${view}` }))
        : [{ id: 'today', url: WALK_URL }, { id: 'plan', url: `${WALK_URL}&ground=plan` }]);
    let state = load();
    const STEPS = stepsOf(state);
    // A start page with ?budget=S gives the session S seconds in place of 170, and its waits grow alike (a software GPU).
    const slow = Math.max(1, (state?.budget ?? 170) / 170);
    const START_MS = 60_000 * slow, IDLE_MS = 10_000 * slow;
    const post = (key, value) => {
        const body = JSON.stringify({ session: state.session, key, value });
        return fetch('/result', { method: 'POST', headers: { 'content-type': 'application/json' }, body, keepalive: body.length < 60_000 }).catch(() => {});
    };
    let events = 0;
    const event = (kind, value) => {
        if (/error|lost|rejected|failed/.test(kind) || value?.found) hooks.errors++;
        return events++ < 600 && post('event', { step: STEPS[state.index]?.id, kind, ...value });
    };
    const blob = (name, body, type = 'application/octet-stream') =>
        fetch(`/blob?session=${state.session}&name=${encodeURIComponent(name)}`, { method: 'POST', headers: { 'content-type': type }, body }).catch(() => {});
    const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
    const within = (promise, ms, what) => Promise.race([promise, sleep(ms).then(() => { throw new Error(`${what}: no answer in ${ms / 1000} s`); })]);
    const fnv = bytes => {
        let a = 0x811c9dc5, b = 0x9e3779b9;
        for (let i = 0; i < bytes.length; i++) { a = Math.imul(a ^ bytes[i], 16777619); b = (Math.imul(b ^ bytes[i], 0x01000193) + 0x7f4a7c15) | 0; }
        return (a >>> 0).toString(16).padStart(8, '0') + (b >>> 0).toString(16).padStart(8, '0');
    };
    const u8 = view => new Uint8Array(view.buffer ?? view, view.byteOffset ?? 0, view.byteLength);

    // --- 1. Which graphics program fails and why ---------------------------------------------------------------
    const modules = new WeakMap();
    const describe = descriptor => {
        const stage = ['compute', 'vertex', 'fragment'].map(name => descriptor?.[name] && `${name}:${descriptor[name].entryPoint ?? '(default)'}@${modules.get(descriptor[name].module)?.name ?? '?'}`).filter(Boolean);
        return { label: descriptor?.label ?? '', stages: stage.join(' ') };
    };
    /** Runs `run` under one error scope per filter: its value, and a promise of the errors that the scopes caught. */
    const scoped = (device, filters, run) => {
        for (const filter of filters) device.pushErrorScope(filter);
        const errors = [], pops = [];
        let value;
        try { value = run(); } finally {
            for (let i = filters.length; i--;) pops.push(device.popErrorScope().then(error => error && errors.push(errorText(filters[i], error))));
        }
        return [value, Promise.all(pops).then(() => errors)];
    };
    const errorText = (filter, error) => ({ filter, type: error?.constructor?.name, message: String(error?.message ?? '') });
    function hook() {
        performance.setResourceTimingBufferSize?.(10_000);
        try {
            new PerformanceObserver(list => {
                for (const e of list.getEntries()) hooks.fetches.push({ layer: new URL(e.name).pathname.split('/')[1], start: e.startTime, ms: e.duration, bytes: e.transferSize || e.encodedBodySize || 0 });
            }).observe({ type: 'resource', buffered: true });
        } catch { /* no resource timing */ }
        addEventListener('error', e => event('page error', { message: e.message, at: `${e.filename}:${e.lineno}:${e.colno}` }));
        addEventListener('unhandledrejection', e => event('unhandled rejection', { message: String(e.reason?.message ?? e.reason), type: e.reason?.constructor?.name }));
        let said = 0;
        for (const level of ['warn', 'error']) {
            const original = console[level];
            console[level] = (...args) => { if (said++ < 60) event('console', { level, text: args.map(String).join(' ').slice(0, 600) }); original.apply(console, args); };
        }
        tapWorkers();
        if (!navigator.gpu) return event('no navigator.gpu', {});
        const D = GPUDevice.prototype;
        Object.assign(original, { createShaderModule: D.createShaderModule, compute: D.createComputePipelineAsync, render: D.createRenderPipelineAsync });
        const requestDevice = GPUAdapter.prototype.requestDevice;
        GPUAdapter.prototype.requestDevice = async function (...args) {
            const device = await requestDevice.apply(this, args);
            event('device', { features: [...device.features].sort() });
            device.addEventListener('uncapturederror', e => event('uncaptured error', errorText('uncaptured', e.error)));
            device.lost.then(info => event('device lost', { reason: info.reason, message: info.message }));
            // The first frames under one scope of each filter: what no single call's scope catches (submits, passes).
            const filters = ['validation', 'internal', 'out-of-memory'];
            for (const filter of filters) device.pushErrorScope(filter);
            hooks.popFirst = () => {
                hooks.popFirst = undefined;
                for (let i = filters.length; i--;) device.popErrorScope().then(error => event('first frames scope', { filter: filters[i], found: !!error, ...(error && errorText(filters[i], error)) }), e => event('first frames scope', { filter: filters[i], popFailed: e.message }));
            };
            setTimeout(() => hooks.popFirst?.(), 25_000 * slow);
            return device;
        };
        // capture() copies the canvas texture: the page configures it without COPY_SRC unless it renders stills.
        const configure = GPUCanvasContext.prototype.configure;
        GPUCanvasContext.prototype.configure = function (config) { return configure.call(this, { ...config, usage: (config.usage ?? GPUTextureUsage.RENDER_ATTACHMENT) | GPUTextureUsage.COPY_SRC }); };
        const createShaderModule = D.createShaderModule;
        D.createShaderModule = function (descriptor) {
            const module = createShaderModule.call(this, descriptor);
            const code = String(descriptor.code), hash = fnv(new TextEncoder().encode(code)).slice(0, 8);
            const name = descriptor.label ? `${descriptor.label}#${hash}` : `#${hash}`;
            modules.set(module, { name, code });
            event('shader module', { name, lines: code.split('\n').length, first: code.split('\n', 1)[0].slice(0, 120) });
            module.getCompilationInfo?.().then(info => {
                const lines = code.split('\n');
                for (const m of info.messages) {
                    const around = m.type === 'error' && m.lineNum ? lines.slice(Math.max(0, m.lineNum - 4), m.lineNum + 3).map((l, i) => `${Math.max(1, m.lineNum - 3) + i}: ${l}`).join('\n') : undefined;
                    event('compilation message', { module: name, type: m.type, line: m.lineNum, column: m.linePos, message: m.message, around });
                }
                if (info.messages.some(m => m.type === 'error')) blob(`shader-${name.replace(/[^\w.-]+/g, '_')}.wgsl`, code, 'text/plain');
            }, e => event('compilation info failed', { module: name, message: e.message }));
            return module;
        };
        for (const kind of ['Compute', 'Render']) {
            const sync = D[`create${kind}Pipeline`], async = D[`create${kind}PipelineAsync`];
            D[`create${kind}Pipeline`] = function (descriptor) {
                const [pipeline, errors] = scoped(this, ['validation', 'internal'], () => sync.call(this, descriptor));
                errors.then(found => found.length && event(`${kind.toLowerCase()} pipeline error`, { ...describe(descriptor), errors: found }));
                return pipeline;
            };
            D[`create${kind}PipelineAsync`] = function (descriptor) {
                hooks.pipelines++;
                return async.call(this, descriptor).catch(error => {
                    const what = describe(descriptor);
                    event(`${kind.toLowerCase()} pipeline rejected`, { ...what, type: error?.constructor?.name, reason: error?.reason, message: String(error?.message ?? error) });
                    // The same descriptor made at once under scopes: its error may carry the text that the rejection lacks.
                    const [, errors] = scoped(this, ['validation', 'internal'], () => sync.call(this, descriptor));
                    errors.then(found => event(`${kind.toLowerCase()} pipeline retried`, { ...what, errors: found }));
                    // Safari names the stage whose library failed; a compute pipeline has one.
                    const stage = kind === 'Compute' ? 'compute' : /fragment/i.test(error?.message) ? 'fragment' : 'vertex';
                    const info = modules.get(descriptor[stage]?.module);
                    if (info) blob(`shader-${info.name.replace(/[^\w.-]+/g, '_')}.wgsl`, info.code, 'text/plain');
                    if (info) enqueue({ device: this, descriptor, stage, module: descriptor[stage].module, code: info.code, entry: descriptor[stage].entryPoint, key: `${info.name} ${stage}:${descriptor[stage].entryPoint}`, label: what.label, async });
                    throw error;
                });
            };
        }
        // Every buffer and texture under an out-of-memory scope, and the first calls of each kind under a validation scope:
        // an error names the object, its size and its usage.
        for (const name of ['createBuffer', 'createTexture', 'createBindGroup', 'createBindGroupLayout', 'createSampler']) {
            const made = D[name], memory = name === 'createBuffer' || name === 'createTexture';
            let calls = 0;
            D[name] = function (descriptor) {
                const filters = [...(calls++ < 300 ? ['validation'] : []), ...(memory ? ['out-of-memory'] : [])];
                if (!filters.length) return made.call(this, descriptor);
                const [object, errors] = scoped(this, filters, () => made.call(this, descriptor));
                errors.then(found => found.length && event(`${name} error`, { label: descriptor?.label ?? '', size: descriptor?.size, format: descriptor?.format, usage: descriptor?.usage, errors: found }));
                return object;
            };
        }
    }
    const hooks = { pipelines: 0, popFirst: undefined, errors: 0, fetches: [] };
    const original = {};

    /** The ground jobs sent before `until`: count, round trip median, p95 and sum in ms, and the ms from the probe to the last of their pages. */
    function groundLoad(until) {
        const { open } = hooks.groundTimes, jobs = hooks.groundTimes.jobs.filter(job => job.start < until), ms = jobs.map(job => job.ms).sort((a, b) => a - b), at = q => Math.round(ms[Math.min(ms.length - 1, Math.floor(q * ms.length))]);
        if (!ms.length) return { jobs: 0 };
        return { jobs: ms.length, median: at(0.5), p95: at(0.95), sum: Math.round(ms.reduce((a, b) => a + b, 0)), openToLast: Math.round(Math.max(...jobs.map(job => job.start + job.ms)) - open) };
    }

    // --- The workers: their errors, and the ground worker's jobs and failures ------------------------------------
    function tapWorkers() {
        const Base = window.Worker;
        let jobs = 0, replies = 0, failures = 0;
        const times = hooks.groundTimes = { open: undefined, jobs: [], inFlight: 0 };
        window.Worker = class extends Base {
            constructor(url, options) {
                super(url, options);
                const name = String(url).split('/').pop().split('?')[0];
                this.addEventListener('error', e => event('worker error', { worker: name, message: e.message, at: `${e.filename}:${e.lineno}` }));
                this.addEventListener('messageerror', () => event('worker message error', { worker: name }));
                if (!name.includes('ground-worker')) return;
                const post = this.postMessage.bind(this), sent = new Map();
                this.postMessage = (data, ...rest) => {
                    // The first message is the decode probe that GroundSource.open sends.
                    const now = performance.now(), job = data?.job;
                    times.open ??= now;
                    if (job) {
                        sent.set(data.id, now);
                        times.inFlight++;
                    }
                    // The bodies of the first job, to decode them on a desktop the same way.
                    if (job && jobs++ === 0 && state.mode !== 'cold') for (const body of [job.ground, ...(job.quarters ?? []), job.snow]) if (body?.body) blob(`ground-${body.layer}-${body.z}-${body.x}-${body.y}.bin`, body.body.slice(0));
                    return post(data, ...rest);
                };
                this.addEventListener('message', ({ data }) => {
                    replies++;
                    if (data?.error && failures++ < 10) event('ground worker failed', { message: String(data.error).slice(0, 400) });
                    const start = sent.get(data?.id);
                    if (start !== undefined) {
                        sent.delete(data.id);
                        times.inFlight--;
                        const now = performance.now(), sentAt = performance.timeOrigin + start, k = data.kit;
                        // phone-worker.js: the wait for the worker, its decodes and page build, and the way back to this thread.
                        const split = k && { wait: k.start - sentAt, work: k.end - k.start, decode: k.decode, decodes: k.decodes, build: k.build, back: performance.timeOrigin + now - k.end };
                        times.jobs.push({ start, ms: now - start, failed: !!data.error, ...split });
                    }
                    hooks.ground = { jobs, replies, failures };
                });
            }
        };
    }

    // --- Bisect: the function of a refused shader that the compiler rejects -----------------------------------------
    /** The functions of composed WGSL (no comments): body range, return type without attributes, and callees. */
    function parse(code) {
        const fns = new Map(), re = /\bfn\s+([A-Za-z_]\w*)\s*\(/g;
        for (let m; (m = re.exec(code));) {
            let i = re.lastIndex, depth = 1;
            while (depth) depth += { '(': 1, ')': -1 }[code[i++]] ?? 0;
            const open = code.indexOf('{', i);
            let j = open + 1;
            depth = 1;
            while (depth) depth += { '{': 1, '}': -1 }[code[j++]] ?? 0;
            fns.set(m[1], { start: m.index, open, close: j, ret: code.slice(i, open).replace(/^\s*->/, '').replace(/@\w+(\([^)]*\))?/g, '').trim() });
            re.lastIndex = j;
        }
        for (const [name, f] of fns) f.calls = new Set([...code.slice(f.open, f.close).matchAll(/\b([A-Za-z_]\w*)\s*\(/g)].map(m => m[1]).filter(n => n !== name && fns.has(n)));
        return fns;
    }
    /** The code with these functions' bodies replaced by a return of the zero value. */
    const stub = (code, fns, names) => [...names].map(n => fns.get(n)).sort((a, b) => b.open - a.open)
        .reduce((out, f) => out.slice(0, f.open) + (f.ret ? `{ return ${f.ret}(); }` : '{ }') + out.slice(f.close), code);
    let queue = Promise.resolve(), bisectUntil = 0;
    const queued = new Set();
    function enqueue(target) {
        if (queued.has(target.key)) return;
        queued.add(target.key);
        hooks.bisects = queue = queue.then(() => bisect(target)).catch(e => event('bisect failed', { key: target.key, message: e.message }));
    }
    async function bisect(t) {
        if (!bisectUntil) bisectUntil = performance.now() + BISECT_S * 1000 * slow;
        const fns = parse(t.code), seen = new Set(), leavesFirst = [];
        const visit = n => { if (seen.has(n)) return; seen.add(n); for (const c of fns.get(n).calls) visit(c); leavesFirst.push(n); };
        if (!fns.has(t.entry)) return event('bisect', { key: t.key, error: 'entry point not found' });
        visit(t.entry);
        const tests = [];
        const compiles = async names => {
            const t0 = performance.now(), code = stub(t.code, fns, names);
            const module = original.createShaderModule.call(t.device, { label: 'bisect', code });
            let result;
            if ((await module.getCompilationInfo()).messages.some(m => m.type === 'error')) result = 'invalid';
            else {
                const d = { ...t.descriptor };
                for (const s of ['compute', 'vertex', 'fragment']) if (d[s]?.module === t.module) d[s] = { ...d[s], module };
                result = await t.async.call(t.device, d).then(() => 'ok', () => 'refused');
            }
            tests.push({ stubbed: names.length <= 4 ? names : [`${names.length}: …`, ...names.slice(-2)], result, ms: Math.round(performance.now() - t0) });
            return result === 'ok';
        };
        const report = extra => event('bisect', { key: t.key, label: t.label, functions: leavesFirst.length, ...extra, tests });
        if (await compiles([])) return report({ verdict: 'compiles when made again' });
        if (!(await compiles([t.entry]))) return report({ verdict: 'refused with the entry point stubbed: a declaration outside the functions' });
        // The shortest prefix of the functions, leaves first, whose stubs let it compile ends at a rejected function:
        // every caller of a function comes after it, so no prefix without it cuts all paths to it.
        const found = [];
        while (found.length < 3 && performance.now() < bisectUntil) {
            if (found.length && (await compiles(found))) break;
            let lo = 0, hi = leavesFirst.length;
            while (hi - lo > 1 && performance.now() < bisectUntil) {
                const mid = (lo + hi) >> 1;
                if (await compiles([...found, ...leavesFirst.slice(0, mid)])) hi = mid;
                else lo = mid;
            }
            if (hi - lo > 1) break;
            found.push(leavesFirst[hi - 1]);
            if (leavesFirst[hi - 1] === t.entry) break;
        }
        const source = found.map(n => t.code.slice(fns.get(n).start, fns.get(n).close).slice(0, 5000));
        report({ verdict: performance.now() < bisectUntil ? 'found' : 'out of time', found, source });
    }

    // --- The page texture (GPU) ----------------------------------------------------------------------------------
    const UNITS = 2 ** 30;
    const worldUnits = (lon, lat) => [((lon + 180) / 360) * UNITS, ((1 - Math.asinh(Math.tan((lat * Math.PI) / 180)) / Math.PI) / 2) * UNITS];
    /** Per drawn page: hashes and the size of its spikes, the 4-neighbour Laplacian in metres; the pages under the eye post raw. */
    async function pagesReport(tv, tag, eye) {
        const terrain = tv.terrain, r = terrain.resources, device = tv.gpu.device;
        const list = [...terrain.pages.pages].filter(([, p]) => p.ready && p.drawn !== undefined);
        const scalesRead = device.createBuffer({ size: r.scales.size, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
        let encoder = device.createCommandEncoder();
        encoder.copyBufferToBuffer(r.scales, 0, scalesRead, 0, r.scales.size);
        device.queue.submit([encoder.finish()]);
        await scalesRead.mapAsync(GPUMapMode.READ);
        const scales = new Float32Array(scalesRead.getMappedRange().slice(0));
        scalesRead.destroy();
        const [ex, ey] = worldUnits(eye.lon, eye.lat);
        const under = ({ level, x, y }) => Math.floor(ex / 2 ** (30 - level)) === x && Math.floor(ey / 2 ** (30 - level)) === y;
        const jobs = [];
        for (const [name, p] of list) {
            jobs.push({ name, p, slot: p.drawn, which: 'drawn' });
            if (p.kind === 'stored' && p.drawn !== p.slot) jobs.push({ name, p, slot: p.slot, which: 'raw' });
        }
        const rows = [], PAGE_BYTES = 256 * 512, BATCH = 48;
        for (let first = 0; first < jobs.length; first += BATCH) {
            const batch = jobs.slice(first, first + BATCH);
            const staging = device.createBuffer({ size: batch.length * PAGE_BYTES, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
            encoder = device.createCommandEncoder();
            batch.forEach((job, i) => encoder.copyTextureToBuffer({ texture: r.heights, origin: [256 * (job.slot & 7), 256 * ((job.slot >> 3) & 7), job.slot >> 6] }, { buffer: staging, offset: i * PAGE_BYTES, bytesPerRow: 512 }, [256, 256, 1]));
            device.queue.submit([encoder.finish()]);
            await staging.mapAsync(GPUMapMode.READ);
            const all = new Uint16Array(staging.getMappedRange());
            for (const [i, job] of batch.entries()) {
                const codes = all.subarray(i * 65536, (i + 1) * 65536), offset = scales[2 * job.slot], scale = scales[2 * job.slot + 1];
                const h = k => offset + codes[k] * scale;
                let known = 0, min = Infinity, max = -Infinity, lap = 0, spikes = 0;
                for (let j = 0; j < 256; j++) for (let x = 0; x < 256; x++) {
                    const k = j * 256 + x;
                    if (codes[k] === 0xffff) continue;
                    known++;
                    const v = h(k);
                    if (v < min) min = v;
                    if (v > max) max = v;
                    if (x === 0 || j === 0 || x === 255 || j === 255) continue;
                    if (codes[k - 1] === 0xffff || codes[k + 1] === 0xffff || codes[k - 256] === 0xffff || codes[k + 256] === 0xffff) continue;
                    const l = Math.abs(4 * v - h(k - 1) - h(k + 1) - h(k - 256) - h(k + 256));
                    if (l > lap) lap = l;
                    if (l > 30) spikes++;
                }
                const row = { name: job.name, which: job.which, kind: job.p.kind, slot: job.slot, copy: job.p.copy, mask: job.p.mask, complete: job.p.complete, offset, scale, hash: fnv(u8(codes)), known, min: round(min), max: round(max), maxLaplacian: round(lap), spikes };
                rows.push(row);
                if (under(job.p.node) && state.mode !== 'cold') blob(`page-${tag}-${job.name.replaceAll('/', '-')}-${job.which}.bin`, codes.slice(0));
            }
            staging.unmap();
            staging.destroy();
        }
        rows.sort((a, b) => a.name.localeCompare(b.name, 'en', { numeric: true }) || a.which.localeCompare(b.which));
        await post(`pages.${tag}`, { eye, count: list.length, rows });
        return rows;
    }
    const round = v => (Number.isFinite(v) ? Math.round(v * 1000) / 1000 : v);

    // --- What the phone draws in flight -------------------------------------------------------------------------
    async function idle(tv) {
        try { await within(tv.waitForIdle(), IDLE_MS, 'waitForIdle'); return true; } catch (e) { event('not idle', { message: e.message }); return false; }
    }
    /** The canvas as the page shows it after the frame that just drew, as a JPEG; no wait for the jitter to converge. */
    function grab(tv, name, extra) {
        const source = document.querySelector('canvas'), canvas = Object.assign(document.createElement('canvas'), { width: source.width, height: source.height });
        const context = canvas.getContext('2d');
        context.drawImage(source, 0, 0);
        const row = context.getImageData(0, canvas.height >> 1, canvas.width, 1).data;
        let sum = 0;
        for (let i = 0; i < row.length; i += 4) sum += row[i] + row[i + 1] + row[i + 2];
        const s = tv.stats();
        const record = { name, blank: sum === 0, view: tv.view, drawn: s.drawn, resident: s.resident, quality: s.terrain.quality, pools: Math.round([...s.pools.values()].reduce((a, b) => a + b, 0) / 2 ** 20), ...extra };
        if (!extra) event('grab', record);
        // A still view that has all its jittered frames draws none, and the canvas reads black: no picture then.
        if (!(extra && record.blank)) canvas.toBlob(jpeg => jpeg && blob(`${name}.jpg`, jpeg, 'image/jpeg'), 'image/jpeg', 0.8);
        return record;
    }
    /** A flight whose pose follows the clock, as phone-measure.js flies, with a picture every `every` seconds. */
    async function fly(tv, name, from, to, seconds, every) {
        tv.view = from;
        await idle(tv);
        const t0 = performance.now();
        for (let shot = 0; ;) {
            const t = Math.min(1, (performance.now() - t0) / (seconds * 1000)), mix = (a, b) => a + (b - a) * t;
            tv.view = { ...from, lat: mix(from.lat, to.lat), lon: mix(from.lon, to.lon), heading: mix(from.heading, to.heading), pitch: mix(from.pitch, to.pitch) };
            await Promise.race([tv.nextFrame(), sleep(5000)]);
            if (performance.now() - t0 >= shot * every * 1000) grab(tv, `${name}-${String(shot++).padStart(2, '0')}`);
            if (t >= 1) break;
        }
    }
    // --- The cold start check -----------------------------------------------------------------------------------
    /** What the engine draws and waits for at a picture: drawn nodes, and those that draw their own page; loads, stamps and ground jobs in flight. */
    function busy(tv) {
        const s = tv.stats(), t = tv.terrain, pages = t.pages.pages, nodes = t.selected?.drawn ?? [];
        let own = 0;
        for (const n of nodes) {
            const page = pages.get(`${n.level}/${n.x}/${n.y}`);
            if (page?.ready && page.drawn !== undefined) own++;
        }
        return {
            nodes: nodes.length, own, finest: Math.max(0, ...nodes.map(n => n.level)), resident: s.resident, loads: t.pages.loads, missing: t.pages.missing,
            arrivals: t.pages.arrivals?.length, left: s.terrain.left.reduce((a, b) => a + b, 0), stamps: s.terrain.jobs, stampsLoading: t.stamps?.loading,
            roadsLoading: t.roads?.loading, near: t.near, eye: t.eye, ground: hooks.groundTimes.inFlight, groundShort: s.ground.shortage, quality: s.terrain.quality,
            cpu: round(tv.cpu), errors: hooks.errors,
        };
    }
    const quantile = (list, q) => { const s = [...list].sort((a, b) => a - b); return s.length ? round(s[Math.min(s.length - 1, Math.floor(q * s.length))]) : undefined; };
    /** The requests per first path segment, before the flight and in it: count, median and p95 ms, bytes. */
    function fetches(flightStart) {
        const out = {};
        for (const f of hooks.fetches) {
            const group = (out[`${f.layer} ${f.start < flightStart ? 'load' : 'flight'}`] ??= { ms: [], bytes: 0 });
            group.ms.push(f.ms);
            group.bytes += f.bytes;
        }
        return Object.fromEntries(Object.entries(out).map(([k, g]) => [k, { count: g.ms.length, median: quantile(g.ms, 0.5), p95: quantile(g.ms, 0.95), bytes: g.bytes }]));
    }
    /** One cold load: a picture each second from the first drawn frame, through the load and the walk flight of phone-measure.js. */
    async function cold(tv, step) {
        const marks = {}, shots = [], intervals = [], key = `cold.${step.id}`;
        const mark = name => () => (marks[name] ??= Math.round(performance.now()));
        tv.eyeReady().then(mark('eyeReady'), e => event('eyeReady failed', { message: e.message }));
        tv.nearReady().then(mark('nearReady'), e => event('nearReady failed', { message: e.message }));
        const shot = (phase, extra) => {
            shots.push(grab(tv, `${step.id}-${String(shots.length).padStart(2, '0')}-${phase}`, { at: Math.round(performance.now()), phase, ...extra, ...busy(tv) }));
            if (shots.length % 5 === 0) post(key, { marks, shots });
        };
        const label = `Step ${state.index + 1} of ${STEPS.length}${step.id === 'cold-off' ? '' : ' (test colours)'}`;
        say(`${label}: loading …`);
        // The engine draws as it wants while it loads; each second one frame is asked for, and the picture taken after it.
        const deadline = performance.now() + COLD_READY_MS;
        for (let due = 0; performance.now() < deadline;) {
            await new Promise(requestAnimationFrame);
            // The first proper render of phone-measure.js, where its flight starts.
            if (marks.nearReady !== undefined && !tv.terrain.meshesLoading) {
                marks.proper = Math.round(performance.now());
                break;
            }
            if (tv.stats().drawn > 0 && performance.now() >= due) {
                await Promise.race([tv.nextFrame(), sleep(5000)]);
                shot('load');
                due = performance.now() + 1000;
            }
        }
        await pagesReport(tv, `${step.id}-ready`, WALK).catch(e => event('report failed', { name: 'pages ready', message: e.message }));
        say(`${label}: the walk`);
        const from = { ...tv.view }, to = { ...from, ...WALK_END }, t0 = Math.round(performance.now());
        for (let n = 0, last; ;) {
            const t = Math.min(1, (performance.now() - t0) / (FLIGHT_S * 1000)), mix = (a, b) => a + (b - a) * t;
            tv.view = { ...from, lat: mix(from.lat, to.lat), lon: mix(from.lon, to.lon), heading: mix(from.heading, to.heading), pitch: mix(from.pitch, to.pitch), height: mix(from.height, to.height) };
            await Promise.race([tv.nextFrame(), sleep(5000)]);
            const now = performance.now();
            if (last !== undefined) intervals.push(Math.round(now - last));
            last = now;
            if (now - t0 >= n * 1000) {
                shot('fly', { t: round((now - t0) / 1000), frame: intervals.at(-1) });
                n++;
            }
            if (t >= 1) break;
        }
        say(`${label}: reading the heights`);
        await pagesReport(tv, `${step.id}-end`, WALK_END).catch(e => event('report failed', { name: 'pages end', message: e.message }));
        const ground = hooks.groundTimes.jobs.map(job => Object.fromEntries(Object.entries(job).map(([k, v]) => [k, typeof v === 'number' ? Math.round(v) : v])));
        await post(key, { marks, flightStart: t0, frame: { median: quantile(intervals, 0.5), p95: quantile(intervals, 0.95), max: Math.max(...intervals) }, intervals, shots, ground, fetches: fetches(t0) });
    }

    const statsOf = tv => JSON.parse(JSON.stringify(tv.stats(), (_, v) => (v instanceof Map ? Object.fromEntries(v) : v)));

    const say = text => {
        let banner = document.getElementById('obc-diagnose');
        if (!banner) {
            banner = Object.assign(document.createElement('div'), { id: 'obc-diagnose' });
            banner.style.cssText = 'position:fixed;left:0;right:0;top:0;z-index:99999;padding:12px;background:rgb(0 0 0 / .7);color:#fff;font:600 18px/1.3 system-ui,sans-serif;pointer-events:none';
            document.body.append(banner);
        }
        banner.textContent = text;
    };
    const left = () => state.deadline - Date.now();
    const notice = () => document.querySelector('.notice')?.textContent;

    async function today(tv) {
        const step = (n, text) => (say(`Step ${n} of 4: ${text}`), post('stage', { step: 'today', at: n, text, elapsed: Date.now() - state.started }));
        step(1, 'a walk');
        const loaded = performance.now(), from = { ...tv.view, debug: 'off' }, to = { ...from, ...WALK_END };
        await fly(tv, 'walk', from, to, 20, 2);
        // The load's jobs, sent before the walk, have their pages by now.
        post('groundLoad', groundLoad(loaded));
        hooks.popFirst?.();
        post('stats.walk', statsOf(tv));
        step(2, 'the walk back with test colours');
        await fly(tv, 'normals', { ...to, debug: 'normals' }, { ...from, debug: 'normals' }, 12, 2);
        tv.view = { ...to, debug: 'off' };
        step(3, 'reading the heights');
        await idle(tv);
        // drawImage reads the canvas only after a frame that draws.
        await Promise.race([tv.nextFrame(), sleep(5000)]);
        grab(tv, 'walk-end-idle');
        try { await pagesReport(tv, 'walk-end', WALK_END); } catch (e) { event('report failed', { name: 'pages', message: e.message }); }
        post('stats.end', statsOf(tv));
        post('ground', hooks.ground ?? { jobs: 0 });
        step(4, 'testing graphics programs');
        try { await within(tv.audit(), 30_000 * slow, 'audit'); } catch (e) { event('audit', { message: e.message }); }
        await within(hooks.bisects ?? Promise.resolve(), (BISECT_S + 10) * 1000 * slow, 'bisects').catch(e => event('bisects', { message: e.message }));
    }

    async function plan() {
        say('Last step: testing the graphics programs of the new roads (up to 3 minutes)');
        await post('stage', { step: 'plan', text: 'bisects', elapsed: Date.now() - state.started });
        await sleep(2000);
        await within(hooks.bisects ?? Promise.resolve(), (BISECT_S + 10) * 1000 * slow, 'bisects').catch(e => event('bisects', { message: e.message }));
    }

    async function run() {
        const step = STEPS[state.index];
        if (state.loaded === state.index) {
            await post(`steps.${step.id}`, { failure: 'the system reloaded the tab during this step' });
            return next();
        }
        state.loaded = state.index;
        save();
        await new Promise(resolve => (document.readyState === 'loading' ? addEventListener('DOMContentLoaded', resolve) : resolve()));
        say(`Starting: ${{ today: "today's app", plan: 'the new roads' }[step.id] ?? `step ${state.index + 1} of ${STEPS.length}`} …`);
        const started = performance.now();
        let tv;
        while (!(tv = window.trailView) && !notice() && performance.now() - started < START_MS) await sleep(100);
        const record = { url: location.href, startMs: Math.round(performance.now() - started), pipelinesAsked: hooks.pipelines };
        if (!tv) {
            // The page's own notice has no canvas to capture: its text and the errors above are the record.
            await post(`steps.${step.id}`, { ...record, notice: notice() ?? null, failure: notice() ? 'the page shows a notice' : `no viewer in ${START_MS / 1000} s` });
            hooks.popFirst?.();
            if (state.mode !== 'cold') await plan();
            return next();
        }
        await post(`steps.${step.id}`, { ...record, adapter: tv.stats().adapter, canvas: [document.querySelector('canvas')?.width, document.querySelector('canvas')?.height] });
        await (step.id === 'today' ? today(tv) : step.id === 'plan' ? plan() : cold(tv, step));
        await post(`steps.${step.id}`, { ...record, done: true, stats: (() => { try { const s = tv.stats(); return { recoveries: s.recoveries, failed: s.failed, quality: s.terrain.quality, pools: Object.fromEntries(s.pools) }; } catch (e) { return { error: e.message }; } })() });
        await sleep(1500);
        next();
    }

    /** The next step, or the end page of the mode. Each load starts cold: the answers are no-store, and the engine's Cache Storage goes. */
    async function next() {
        state.index++;
        save();
        try { for (const name of await caches.keys()) await caches.delete(name); } catch { /* no Cache Storage */ }
        location.replace(state.index < STEPS.length ? STEPS[state.index].url : `/phone/${state.mode ?? 'diagnose'}`);
    }

    async function home(mode) {
        await new Promise(resolve => (document.readyState === 'loading' ? addEventListener('DOMContentLoaded', resolve) : resolve()));
        document.body.style.cssText = 'margin:0;padding:24px 16px;font:20px/1.4 system-ui,sans-serif;background:#fff;color:#111';
        const big = text => Object.assign(document.createElement('p'), { textContent: text });
        if ((state?.mode ?? 'diagnose') === mode && state.index >= STEPS.length) {
            await post('finished', { elapsed: Date.now() - state.started });
            document.body.replaceChildren(big('Done. Thank you. You can close this page.'), big(`Session ${state.session}`));
            return;
        }
        const button = Object.assign(document.createElement('button'), { textContent: 'Start' });
        button.style.cssText = 'display:block;width:100%;padding:32px;font:700 32px system-ui;border-radius:16px;border:0;background:#1a7f37;color:#fff';
        const what = mode === 'cold' ? 'TrailView cold start check on this phone. It takes about 3 minutes.' : 'TrailView fault check on this phone. It takes about 5 minutes.';
        document.body.replaceChildren(big(what), big('Tap Start, put the phone down and do not touch it until it says "Done".'), button);
        button.onclick = async () => {
            button.disabled = true;
            const query = new URLSearchParams(location.search), budget = +(query.get('budget') ?? 170);
            const views = (query.get('views') ?? 'off,normals').split(',').filter(view => /^[a-z]+$/.test(view));
            state = { session: `${new Date().toISOString().slice(0, 19).replace(/[-:T]/g, '')}-${mode[0]}${Math.random().toString(36).slice(2, 5)}`, mode, views, index: 0, started: Date.now(), budget, deadline: Date.now() + budget * 1000 };
            save();
            // A measuring session left in this tab would take over the TrailView page.
            try { sessionStorage.removeItem('obc-phone'); } catch { /* no storage */ }
            try { await navigator.wakeLock?.request('screen'); } catch { /* Auto-Lock "Never" keeps the screen on */ }
            await post('device', await facts(mode));
            try { for (const name of await caches.keys()) await caches.delete(name); } catch { /* no Cache Storage */ }
            location.replace(stepsOf(state)[0].url);
        };
    }

    async function facts(mode) {
        const f = { mode, userAgent: navigator.userAgent, screen: [screen.width, screen.height], viewport: [innerWidth, innerHeight], pixelRatio: devicePixelRatio, secure: isSecureContext };
        try {
            const adapter = await navigator.gpu.requestAdapter(), info = adapter.info ?? {}, limits = {};
            for (const name in adapter.limits) limits[name] = adapter.limits[name];
            f.adapter = { info: [info.vendor, info.architecture, info.device, info.description].filter(Boolean).join(' '), features: [...adapter.features].sort(), limits };
            f.format = navigator.gpu.getPreferredCanvasFormat();
            f.wgslFeatures = [...(navigator.gpu.wgslLanguageFeatures ?? [])].sort();
        } catch (e) {
            f.adapter = { error: e.message };
        }
        return f;
    }

    const page = /^\/phone\/(diagnose|cold)/.exec(location.pathname)?.[1];
    if (page) return void home(page);
    if (!state || state.index >= STEPS.length || !location.pathname.endsWith('/trailview.html')) return;
    hook();
    run().catch(e => {
        event('the diagnosis script failed', { message: e.message, stack: String(e.stack).slice(0, 800) });
        next();
    });
})();
