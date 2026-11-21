// The compile test of phone.mjs (/phone/compile): whether the phone's shader compiler fails on the size of a shader. Each
// test is a fresh page load, so a failure cannot poison the next. A test compiles one refused compute shader of the
// fault check (served from RESULTS/shaders/) with some of its functions stubbed, down to a graded share of its inlined
// size: every call site inlines its callee, as Metal does. It posts the source bytes, the functions, the inlined
// bytes, the compile time, the result and the errors. Controls: a tiny shader after a refused big one in the same
// load, and in a fresh load. `node phone-compile.js FILE.wgsl ENTRY` prints the graded stub sets without a browser.
(() => {
    /** The functions of composed WGSL without comments: body range, return type without attributes, call sites. */
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
        for (const [name, f] of fns) {
            f.sites = new Map();
            for (const [, callee] of code.slice(f.open, f.close).matchAll(/\b([A-Za-z_]\w*)\s*\(/g)) if (callee !== name && fns.has(callee)) f.sites.set(callee, (f.sites.get(callee) ?? 0) + 1);
        }
        return fns;
    }
    const stub = (code, fns, names) => [...names].map(n => fns.get(n)).sort((a, b) => b.open - a.open)
        .reduce((out, f) => out.slice(0, f.open) + (f.ret ? `{ return ${f.ret}(); }` : '{ }') + out.slice(f.close), code);
    /** The bytes of an entry point with every call inlined, with these functions stubbed. */
    function inlined(fns, entry, stubbed) {
        const memo = new Map();
        const cost = n => {
            if (stubbed.has(n)) return 20;
            if (!memo.has(n)) {
                const f = fns.get(n);
                let c = f.close - f.open;
                for (const [callee, k] of f.sites) c += k * cost(callee);
                memo.set(n, c);
            }
            return memo.get(n);
        };
        return cost(entry);
    }
    /** Stubs to bring the inlined bytes of `entry` down to about `share` of the whole: each time the function whose stub comes nearest. */
    function graded(fns, entry, share) {
        const stubbed = new Set(), target = share * inlined(fns, entry, stubbed);
        for (let total = inlined(fns, entry, stubbed); total > target * 1.05;) {
            let best, bestTotal;
            for (const n of fns.keys()) {
                if (n === entry || stubbed.has(n)) continue;
                stubbed.add(n);
                const t = inlined(fns, entry, stubbed);
                stubbed.delete(n);
                if (t < total && (bestTotal === undefined || Math.abs(t - target) < Math.abs(bestTotal - target))) [best, bestTotal] = [n, t];
            }
            if (!best) break;
            stubbed.add(best);
            total = bestTotal;
        }
        return stubbed;
    }
    const TINY = '@group(0) @binding(0) var<storage, read_write> out: array<u32>;\n@compute @workgroup_size(1)\nfn tiny() {\n    out[0] = 1u;\n}\n';

    if (typeof window === 'undefined') {
        const { readFileSync } = require('node:fs');
        const [file, entry] = process.argv.slice(2), code = readFileSync(file, 'utf8'), fns = parse(code), whole = inlined(fns, entry, new Set());
        for (const share of [1, 0.7, 0.5, 0.35, 0.2]) {
            const stubbed = graded(fns, entry, share), out = stub(code, fns, stubbed);
            console.log(`share ${share}: ${stubbed.size} stubbed, inlined ${inlined(fns, entry, stubbed)} of ${whole}, source ${out.length} B; parse again ${parse(out).size} fns`);
        }
        return;
    }

    // The most telling first: a refused shader with the control after it; the control alone; then the grades.
    const TESTS = [
        { shader: 'audit', entry: 'audit', share: 1, control: true },
        { tiny: true },
        { shader: 'audit', entry: 'audit', share: 0.7 },
        { shader: 'audit', entry: 'audit', share: 0.45 },
        { shader: 'audit', entry: 'audit', share: 0.2 },
        { shader: 'stamp', entry: 'stamp', share: 1, constants: { MODEL: 0 } },
        { shader: 'stamp', entry: 'stamp', share: 0.5, constants: { MODEL: 1 } },
        { shader: 'stamp', entry: 'stamp', share: 0.25, constants: { MODEL: 1 } },
        { shader: 'stamp', entry: 'stamp', share: 1, constants: { MODEL: 1 }, control: true },
    ];
    const KEY = 'obc-compile';
    let state;
    try { state = JSON.parse(sessionStorage.getItem(KEY)) ?? undefined; } catch { /* none */ }
    const save = () => { try { sessionStorage.setItem(KEY, JSON.stringify(state)); } catch { /* none */ } };
    const post = (key, value) => fetch('/result', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ session: state.session, key, value }) }).catch(() => {});
    const ready = new Promise(resolve => (document.readyState === 'loading' ? addEventListener('DOMContentLoaded', resolve) : resolve()));
    const say = async text => { await ready; document.body.style.cssText = 'margin:0;padding:24px 16px;font:20px/1.4 system-ui,sans-serif'; document.body.textContent = text; };

    /** One compile under error scopes, with the device's uncaptured errors in the meantime. */
    async function compile(device, code, entry, constants) {
        const errors = [], seen = e => errors.push({ type: e.error?.constructor?.name, message: String(e.error?.message ?? '') });
        device.addEventListener('uncapturederror', seen);
        for (const filter of ['validation', 'internal', 'out-of-memory']) device.pushErrorScope(filter);
        const t0 = performance.now(), module = device.createShaderModule({ code });
        const invalid = (await module.getCompilationInfo()).messages.filter(m => m.type === 'error').map(m => `${m.lineNum}:${m.linePos} ${m.message}`);
        const result = invalid.length ? 'invalid WGSL' : await device.createComputePipelineAsync({ layout: 'auto', compute: { module, entryPoint: entry, constants } })
            .then(() => 'ok', e => `refused: ${e.constructor?.name} ${e.reason ?? ''} ${e.message}`);
        const ms = Math.round(performance.now() - t0);
        for (const filter of ['out-of-memory', 'internal', 'validation']) {
            const error = await device.popErrorScope();
            if (error) errors.push({ filter, type: error.constructor?.name, message: error.message });
        }
        device.removeEventListener('uncapturederror', seen);
        return { result, ms, errors, invalid: invalid.slice(0, 3) };
    }

    async function test() {
        const i = state.index, t = TESTS[i];
        await say(`Compile test ${i + 1} of ${TESTS.length} … please wait`);
        const record = { test: i + 1, ...t };
        await post(`tests.${i}`, { ...record, stage: 'started' });
        try {
            const device = await (await navigator.gpu.requestAdapter()).requestDevice();
            device.lost.then(info => post(`lost.${i}`, { reason: info.reason, message: info.message }));
            if (t.shader) {
                const code = await (await fetch(`/phone/shaders/${t.shader}.wgsl`)).text(), fns = parse(code), stubbed = graded(fns, t.entry, t.share), out = stub(code, fns, stubbed);
                Object.assign(record, { sourceBytes: out.length, functions: parse(out).size, stubbed: stubbed.size, inlinedBytes: inlined(fns, t.entry, stubbed), wholeInlinedBytes: inlined(fns, t.entry, new Set()) });
                await post(`tests.${i}`, { ...record, stage: 'compiling' });
                record.big = await compile(device, out, t.entry, t.constants);
            }
            if (t.tiny || t.control) record.tiny = await compile(device, TINY, 'tiny');
        } catch (e) {
            record.failure = String(e.message ?? e);
        }
        await post(`tests.${i}`, { ...record, stage: 'done' });
        state.index++;
        save();
        location.replace(state.index < TESTS.length ? '/phone/compile' : '/phone/compile?done');
    }

    async function home() {
        await ready;
        if (state && state.index >= TESTS.length) return say(`Done. Thank you. Session ${state.session}`);
        if (state && state.index >= 0) return test();
        const button = Object.assign(document.createElement('button'), { textContent: 'Start' });
        button.style.cssText = 'display:block;width:100%;padding:32px;font:700 32px system-ui;border-radius:16px;border:0;background:#1a7f37;color:#fff';
        document.body.replaceChildren(Object.assign(document.createElement('p'), { textContent: 'TrailView compile test. About 3 minutes. Tap Start and do not touch the phone until it says "Done".' }), button);
        button.onclick = async () => {
            button.disabled = true;
            state = { session: `${new Date().toISOString().slice(0, 19).replace(/[-:T]/g, '')}-c${Math.random().toString(36).slice(2, 5)}`, index: 0 };
            save();
            const a = await navigator.gpu?.requestAdapter();
            await post('device', { userAgent: navigator.userAgent, adapter: a && [a.info?.vendor, a.info?.architecture].join(' '), tests: TESTS });
            location.replace('/phone/compile');
        };
    }
    home();
})();
