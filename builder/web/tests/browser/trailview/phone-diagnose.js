// The diagnosis script of phone.mjs: /phone/diagnose draws its start page; in the TrailView page it runs one
// session of two loads (today's app, then ground=plan), each at the walk view of flights.json. It reports which
// graphics program fails and why, pictures of the view and its debug views from the walk and from above, and the
// terrain's heights at three stages: the decode worker's pages (CPU), the page texture (GPU) and drawnHeight (probe
// and instance shader). Each finding posts at once, so a tab that the system kills leaves its data. A classic
// script, injected before the page's module: it wraps the WebGPU calls first.
(() => {
    const KEY = 'obc-diagnose';
    const TAIL = '&date=2026-08-15&time=15:00&weather=sunny';
    const WALK = { lat: 46.8205, lon: 8.403, heading: 160, pitch: 0, eye: 1.6 };
    const WALK_URL = `/trailview.html?lat=${WALK.lat}&lon=${WALK.lon}&heading=${WALK.heading}&pitch=${WALK.pitch}&eye=${WALK.eye}${TAIL}`;
    const STEPS = [{ id: 'today', url: WALK_URL }, { id: 'plan', url: `${WALK_URL}&ground=plan` }];
    // In the order of their worth, for a session that runs out of time.
    const DEBUG = ['surface', 'normals', 'lod', 'material'];
    /** Decoded terrain tiles whose bodies post, and whose summaries post. */
    const BODIES = 6, SUMMARIES = 40;

    const load = () => { try { return JSON.parse(sessionStorage.getItem(KEY)) ?? undefined; } catch { return undefined; } };
    const save = () => { try { sessionStorage.setItem(KEY, JSON.stringify(state)); } catch { /* the next load starts again */ } };
    let state = load();
    // A start page with ?budget=S gives the session S seconds in place of 170, and its waits grow alike (a software GPU).
    const slow = Math.max(1, (state?.budget ?? 170) / 170);
    const START_MS = 60_000 * slow, IDLE_MS = 20_000 * slow, CAPTURE_MS = 15_000 * slow;
    const post = (key, value) => {
        const body = JSON.stringify({ session: state.session, key, value });
        return fetch('/result', { method: 'POST', headers: { 'content-type': 'application/json' }, body, keepalive: body.length < 60_000 }).catch(() => {});
    };
    let events = 0;
    const event = (kind, value) => events++ < 400 && post('event', { step: STEPS[state.index]?.id, kind, ...value });
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
        addEventListener('error', e => event('page error', { message: e.message, at: `${e.filename}:${e.lineno}:${e.colno}` }));
        addEventListener('unhandledrejection', e => event('unhandled rejection', { message: String(e.reason?.message ?? e.reason), type: e.reason?.constructor?.name }));
        if (!navigator.gpu) return event('no navigator.gpu', {});
        const D = GPUDevice.prototype;
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
                    const source = descriptor[kind === 'Compute' ? 'compute' : 'vertex']?.module;
                    const info = source && modules.get(source);
                    if (info) blob(`shader-${info.name.replace(/[^\w.-]+/g, '_')}.wgsl`, info.code, 'text/plain');
                    throw error;
                });
            };
        }
        // Each of the first calls under one validation scope: where a later draw fails, these name the object.
        for (const name of ['createBindGroup', 'createBindGroupLayout', 'createTexture', 'createSampler']) {
            const original = D[name];
            let calls = 0;
            D[name] = function (descriptor) {
                if (calls++ > 300) return original.call(this, descriptor);
                this.pushErrorScope('validation');
                let made;
                try { made = original.call(this, descriptor); } finally {
                    this.popErrorScope().then(error => error && event(`${name} error`, { label: descriptor?.label ?? '', message: error.message, format: descriptor?.format, usage: descriptor?.usage }));
                }
                return made;
            };
        }
        tapDecodes();
    }
    const hooks = { pipelines: 0, popFirst: undefined };

    // --- 3a. The decode worker's pages (CPU) -----------------------------------------------------------------------
    const decodes = [];
    function tapDecodes() {
        const Base = window.Worker;
        window.Worker = class extends Base {
            constructor(url, options) {
                super(url, options);
                if (!String(url).includes('decode-worker')) return;
                const bodies = new Map(), post = this.postMessage.bind(this);
                this.postMessage = (data, ...rest) => {
                    if (data?.body instanceof ArrayBuffer) bodies.set(data.id, data.body.slice(0));
                    return post(data, ...rest);
                };
                this.addEventListener('message', ({ data }) => {
                    const body = bodies.get(data?.id);
                    bodies.delete(data?.id);
                    if (!body) return;
                    if (data.error) return event('decode failed', { message: data.error });
                    const bytes = new Uint8Array(body), hash = fnv(bytes);
                    const pages = data.pages.map(p => p && { offset: p.offset, scale: p.scale, min: p.min, max: p.max, complete: p.complete, texels: fnv(u8(p.texels)), samples: [0, 257 * 64, 257 * 128, 257 * 192, 65535].map(i => p.texels[i]) });
                    decodes.push({ hash, body, pages: data.pages });
                    if (decodes.length <= SUMMARIES) event('decoded tile', { body: hash, bytes: bytes.length, pages });
                    if (decodes.length <= BODIES) blob(`tile-${hash}.webp`, body, 'image/webp');
                });
            }
        };
    }

    // The first decoded body again on the main thread, three ways: the engine's options, the defaults, and an image element.
    async function decodeVariants() {
        const first = decodes.find(d => d.pages[0]);
        if (!first) return post('decodeVariants', { error: 'no decoded tile with a north-west page' });
        const reference = first.pages[0];
        const toRgba = async bitmap => {
            const canvas = Object.assign(document.createElement('canvas'), { width: bitmap.width, height: bitmap.height });
            const context = canvas.getContext('2d', { willReadFrequently: true });
            context.drawImage(bitmap, 0, 0);
            return context.getImageData(0, 0, bitmap.width, bitmap.height);
        };
        const variants = {
            'bitmap-engine-options': async () => toRgba(await createImageBitmap(new Blob([first.body]), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' })),
            'bitmap-defaults': async () => toRgba(await createImageBitmap(new Blob([first.body]))),
            'image-element': async () => {
                const image = new Image(), url = URL.createObjectURL(new Blob([first.body], { type: 'image/webp' }));
                image.src = url;
                await image.decode();
                URL.revokeObjectURL(url);
                return toRgba(image);
            },
        };
        const out = { body: first.hash, worker: { offset: reference.offset, scale: reference.scale } };
        for (const [name, run] of Object.entries(variants)) {
            try {
                const rgba = await run(), d = rgba.data, size = rgba.width;
                let opaque = 0, largest = 0, differ = 0, unknownDiffer = 0;
                for (let j = 0; j < 256; j++) for (let i = 0; i < 256; i++) {
                    const p = 4 * (j * size + i), known = d[p + 3] === 255, h = d[p] * 256 + d[p + 1] + d[p + 2] / 256 - 32768;
                    if (known) opaque++;
                    const code = reference.texels[j * 256 + i];
                    if ((code === 0xffff) !== !known) { unknownDiffer++; continue; }
                    if (!known) continue;
                    const e = Math.abs(reference.offset + code * reference.scale - h);
                    if (e > largest) largest = e;
                    if (e > 0.5) differ++;
                }
                out[name] = { size: [rgba.width, rgba.height], rgba: fnv(d), opaque, largestMetres: largest, over05m: differ, unknownDiffer, centre: Array.from(d.slice(4 * (128 * size + 128), 4 * (128 * size + 128) + 4)) };
            } catch (e) {
                out[name] = { error: e.message };
            }
        }
        await post('decodeVariants', out);
    }

    // --- 3b. The page texture (GPU) -------------------------------------------------------------------------------
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
                if (under(job.p.node)) blob(`page-${tag}-${job.name.replaceAll('/', '-')}-${job.which}.bin`, codes.slice(0));
            }
            staging.unmap();
            staging.destroy();
        }
        rows.sort((a, b) => a.name.localeCompare(b.name, 'en', { numeric: true }) || a.which.localeCompare(b.which));
        await post(`pages.${tag}`, { eye, count: list.length, rows });
        return rows;
    }
    const round = v => (Number.isFinite(v) ? Math.round(v * 1000) / 1000 : v);

    // --- 3c. drawnHeight: the probe (compute) and an instance shader (vertex), on a grid about the eye ---------------
    async function heightsReport(tv, tag, eye) {
        const points = [];
        const dLat = 160 / 111_320, dLon = 160 / (111_320 * Math.cos((eye.lat * Math.PI) / 180));
        for (let j = 0; j < 8; j++) for (let i = 0; i < 8; i++) points.push([eye.lon + ((i - 3.5) / 3.5) * dLon, eye.lat + ((j - 3.5) / 3.5) * dLat]);
        const { probe, instance } = await within(tv.check(points), 10_000 * slow, 'check');
        const differ = probe.map((p, i) => round(instance[i] - p));
        await post(`heights.${tag}`, { points, probe: probe.map(round), instance: instance.map(round), largestDifference: Math.max(...differ.map(Math.abs).filter(Number.isFinite)) });
        return probe;
    }

    // --- 2. What the phone draws -------------------------------------------------------------------------------
    async function idle(tv) {
        try { await within(tv.waitForIdle(), IDLE_MS, 'waitForIdle'); return true; } catch (e) { event('not idle', { message: e.message }); return false; }
    }
    async function shot(tv, name) {
        const idleAt = performance.now(), wasIdle = await idle(tv), waited = Math.round(performance.now() - idleAt);
        const jpeg = canvas => new Promise(resolve => canvas.toBlob(resolve, 'image/jpeg', 0.85));
        try {
            // capture() waits until the jittered frames converge; a view that never settles never gives one.
            const { colour } = await within(tv.capture(), CAPTURE_MS, 'capture');
            let sum = 0;
            for (let i = 0; i < colour.data.length; i += 4 * 97) sum += colour.data[i] + colour.data[i + 1] + colour.data[i + 2];
            if (sum === 0) throw new Error('capture: the copy is black');
            const canvas = Object.assign(document.createElement('canvas'), { width: colour.width, height: colour.height });
            canvas.getContext('2d').putImageData(colour, 0, 0);
            await blob(`${name}.jpg`, await jpeg(canvas), 'image/jpeg');
            event('shot', { name, idle: wasIdle, waitedMs: waited, how: 'capture', size: [colour.width, colour.height], view: tv.view });
        } catch (e) {
            // The canvas as the page shows it, without waiting for the jitter to converge.
            try {
                const source = document.querySelector('canvas'), canvas = Object.assign(document.createElement('canvas'), { width: source.width, height: source.height });
                canvas.getContext('2d').drawImage(source, 0, 0);
                await blob(`${name}.jpg`, await jpeg(canvas), 'image/jpeg');
                event('shot', { name, idle: wasIdle, waitedMs: waited, how: 'canvas', captureFailed: e.message, size: [canvas.width, canvas.height], view: tv.view });
            } catch (f) {
                event('shot failed', { name, message: `${e.message}; canvas: ${f.message}` });
            }
        }
    }
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
        step(1, 'the walk view');
        await shot(tv, 'today-walk');
        hooks.popFirst?.();
        step(2, 'reading the heights');
        const tasks = [['pages', () => pagesReport(tv, 'walk', WALK)], ['heights', () => heightsReport(tv, 'walk', WALK)], ['decode', decodeVariants], ['audit', async () => post('audit.walk', await within(tv.audit(), 30_000 * slow, 'audit'))]];
        for (const [name, run] of tasks) {
            try { await run(); } catch (e) { event('report failed', { name, message: e.message }); }
        }
        step(3, 'debug views from the walk');
        const walk = { ...tv.view };
        for (const debug of DEBUG) {
            if (left() < 60_000 * slow) { event('skipped', { name: `today-walk-${debug}`, reason: 'time' }); continue; }
            tv.view = { ...walk, debug };
            await shot(tv, `today-walk-${debug}`);
        }
        step(4, 'the views from above');
        let ground = WALK.eye;
        try { ground = (await within(tv.check([[WALK.lon, WALK.lat]]), 10_000 * slow, 'check')).probe[0]; } catch (e) { event('ground probe failed', { message: e.message }); }
        const above = { ...walk, mode: 'fly', height: (Number.isFinite(ground) ? ground : 600) + 300, pitch: -60, debug: 'off' };
        tv.view = above;
        await shot(tv, 'today-above');
        try { await pagesReport(tv, 'above', WALK); } catch (e) { event('report failed', { name: 'pages above', message: e.message }); }
        for (const debug of DEBUG) {
            if (left() < 30_000 * slow) { event('skipped', { name: `today-above-${debug}`, reason: 'time' }); continue; }
            tv.view = { ...above, debug };
            await shot(tv, `today-above-${debug}`);
        }
    }

    async function plan(tv) {
        say('Last step: the new roads');
        await post('stage', { step: 'plan', text: 'the new roads', elapsed: Date.now() - state.started });
        await shot(tv, 'plan-walk');
        hooks.popFirst?.();
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
        say(`Starting: ${step.id === 'today' ? "today's app" : 'the new roads'} …`);
        const started = performance.now();
        let tv;
        while (!(tv = window.trailView) && !notice() && performance.now() - started < START_MS) await sleep(100);
        const record = { url: location.href, startMs: Math.round(performance.now() - started), pipelinesAsked: hooks.pipelines };
        if (!tv) {
            // The page's own notice has no canvas to capture: its text and the errors above are the record.
            await post(`steps.${step.id}`, { ...record, notice: notice() ?? null, failure: notice() ? 'the page shows a notice' : `no viewer in ${START_MS / 1000} s` });
            hooks.popFirst?.();
            await sleep(3000);
            return next();
        }
        await post(`steps.${step.id}`, { ...record, adapter: tv.stats().adapter, canvas: [document.querySelector('canvas')?.width, document.querySelector('canvas')?.height] });
        await (step.id === 'today' ? today(tv) : plan(tv));
        await post(`steps.${step.id}`, { ...record, done: true, stats: (() => { try { const s = tv.stats(); return { recoveries: s.recoveries, failed: s.failed, quality: s.terrain.quality, pools: Object.fromEntries(s.pools) }; } catch (e) { return { error: e.message }; } })() });
        await sleep(1500);
        next();
    }

    function next() {
        state.index++;
        save();
        location.replace(state.index < STEPS.length ? STEPS[state.index].url : '/phone/diagnose');
    }

    async function home() {
        await new Promise(resolve => (document.readyState === 'loading' ? addEventListener('DOMContentLoaded', resolve) : resolve()));
        document.body.style.cssText = 'margin:0;padding:24px 16px;font:20px/1.4 system-ui,sans-serif;background:#fff;color:#111';
        const big = text => Object.assign(document.createElement('p'), { textContent: text });
        if (state?.index >= STEPS.length) {
            await post('finished', { elapsed: Date.now() - state.started });
            document.body.replaceChildren(big('Done. Thank you. You can close this page.'), big(`Session ${state.session}`));
            return;
        }
        const button = Object.assign(document.createElement('button'), { textContent: 'Start' });
        button.style.cssText = 'display:block;width:100%;padding:32px;font:700 32px system-ui;border-radius:16px;border:0;background:#1a7f37;color:#fff';
        document.body.replaceChildren(big('TrailView fault check on this phone. It takes about 3 minutes.'), big('Tap Start, put the phone down and do not touch it until it says "Done".'), button);
        button.onclick = async () => {
            button.disabled = true;
            const budget = +(new URLSearchParams(location.search).get('budget') ?? 170);
            state = { session: `${new Date().toISOString().slice(0, 19).replace(/[-:T]/g, '')}-d${Math.random().toString(36).slice(2, 5)}`, index: 0, started: Date.now(), budget, deadline: Date.now() + budget * 1000 };
            save();
            // A measuring session left in this tab would take over the TrailView page.
            try { sessionStorage.removeItem('obc-phone'); } catch { /* no storage */ }
            try { await navigator.wakeLock?.request('screen'); } catch { /* Auto-Lock "Never" keeps the screen on */ }
            await post('device', await facts());
            location.replace(STEPS[0].url);
        };
    }

    async function facts() {
        const f = { mode: 'diagnose', userAgent: navigator.userAgent, screen: [screen.width, screen.height], viewport: [innerWidth, innerHeight], pixelRatio: devicePixelRatio, secure: isSecureContext };
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

    if (location.pathname.startsWith('/phone/diagnose')) return void home();
    if (!state || state.index >= STEPS.length || !location.pathname.endsWith('/trailview.html')) return;
    hook();
    run().catch(e => {
        event('the diagnosis script failed', { message: e.message, stack: String(e.stack).slice(0, 800) });
        next();
    });
})();
