// Measures the built TrailView page on a phone over the home Wi-Fi: one HTTPS origin serves the page, the data of a
// local release, the measuring script (phone-measure.js) and a collector that writes each session to a JSON file.
//
//   node phone.mjs build --origin https://IP:PORT                  # the page and the timing page, into dist/trailview-phone
//   node phone.mjs serve --host IP [--port 8770] [--data http://127.0.0.1:8710] [--results DIR] [--cert DIR] [--requests DIR]
//   node phone.mjs summarize RESULT.json
//
// The phone opens https://IP:PORT/phone/ to measure, or https://IP:PORT/phone/diagnose for the fault check of
// phone-diagnose.js, whose pictures and page bytes land in RESULTS/<session>/, or https://IP:PORT/phone/cold for its cold
// start check (pictures of a cold load and the walk flight; phone-worker.js times the ground worker), or https://IP:PORT/phone/compile for the
// compile test of phone-compile.js over RESULTS/shaders/. --data is the origin of `obc data serve` (or a stand-in), whose catalogue
// names its own origin: the proxy writes the phone's origin in its place. --requests is a folder of the ground plan's
// `requests` tool: the session then ends with the timing page on those cells. Every answer is `no-store`, so each run
// loads cold over the network.
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync, renameSync } from 'node:fs';
import { request as httpRequest } from 'node:http';
import { createServer } from 'node:https';
import { extname, join, normalize, resolve } from 'node:path';
import { parseArgs } from 'node:util';

const HERE = new URL('.', import.meta.url).pathname;
const WEB = resolve(HERE, '../../..');
const DIST = join(WEB, 'dist/trailview-phone');
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.json': 'application/json', '.svg': 'image/svg+xml', '.webp': 'image/webp', '.wasm': 'application/wasm', '.txt': 'text/plain', '.bin': 'application/octet-stream' };
const { values: a, positionals: [command, file] } = parseArgs({
    allowPositionals: true,
    options: {
        origin: { type: 'string' }, host: { type: 'string' }, port: { type: 'string', default: '8770' },
        data: { type: 'string', default: 'http://127.0.0.1:8710' }, results: { type: 'string', default: join(WEB, '.artifacts/phone') },
        cert: { type: 'string', default: join(WEB, '.artifacts/phone/cert') }, requests: { type: 'string' },
    },
});

if (command === 'build') build();
else if (command === 'serve') serve();
else if (command === 'summarize') summarize(JSON.parse(readFileSync(file, 'utf8')));
else throw new Error('Give build, serve or summarize.');

/** The TrailView page and the plan timing page, against the catalogue of the phone's origin. */
async function build() {
    if (!a.origin) throw new Error('Give --origin https://IP:PORT');
    process.env.VITE_PLANNER_CATALOG = `${a.origin}/planner/catalog.json`;
    const { build: vite } = await import(join(WEB, 'node_modules/vite/dist/node/index.js'));
    // The inline config merges into vite.config.ts: the input list adds the timing page to trailview.html.
    await vite({ root: WEB, configFile: join(WEB, 'vite.config.ts'), mode: 'trailview', build: { outDir: DIST, rollupOptions: { input: ['plan-timing.html'] } } });
}

/** A self-signed certificate for the LAN address; iOS trusts it only as a CA with serverAuth and at most 825 days. */
function certificate(dir, host) {
    const key = join(dir, 'key.pem'), cert = join(dir, 'cert.pem');
    if (!existsSync(cert)) {
        mkdirSync(dir, { recursive: true });
        execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '365', '-keyout', key, '-out', cert, '-subj', '/CN=OpenBikeComputer phone kit',
            '-addext', `subjectAltName=IP:${host}`, '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,digitalSignature,keyEncipherment,keyCertSign',
            '-addext', 'extendedKeyUsage=serverAuth']);
    }
    return { key: readFileSync(key), cert: readFileSync(cert) };
}

function serve() {
    if (!a.host) throw new Error('Give --host, the LAN address of this machine');
    const tls = certificate(a.cert, a.host), origin = `https://${a.host}:${a.port}`;
    const requests = a.requests && resolve(a.requests);
    mkdirSync(a.results, { recursive: true });
    const send = (response, status, type, body) => response.writeHead(status, { 'content-type': type, 'cache-control': 'no-store' }).end(body);
    createServer(tls, (request, response) => {
        const path = normalize(decodeURIComponent(new URL(request.url, origin).pathname));
        if (request.method === 'POST' && path === '/result') return collect(request, response);
        if (request.method === 'POST' && path === '/blob') return keep(request, response);
        if (path === '/phone/' || path === '/phone') return send(response, 200, 'text/html', page('measure'));
        if (path === '/phone/diagnose' || path === '/phone/cold') return send(response, 200, 'text/html', page('diagnose'));
        // A phone keyboard adds a capital or a slash.
        if (path === '/phone/measure.js') return send(response, 200, 'text/javascript', readFileSync(join(HERE, 'phone-measure.js')));
        if (path === '/phone/diagnose.js') return send(response, 200, 'text/javascript', readFileSync(join(HERE, 'phone-diagnose.js')));
        if (path === '/phone/compile') return send(response, 200, 'text/html', page('compile'));
        const page_ = /^\/phone\/(diagnos(?:e|is)|compile|cold)\/?$/i.exec(path);
        const name = page_?.[1].toLowerCase();
        if (name) return response.writeHead(302, { location: `/phone/${name.startsWith('diagnos') ? 'diagnose' : name}` }).end();
        if (path === '/phone/compile.js') return send(response, 200, 'text/javascript', readFileSync(join(HERE, 'phone-compile.js')));
        // The shaders of the compile test, as the fault check saved them.
        const shader = /^\/phone\/shaders\/([\w-]+\.wgsl)$/.exec(path), shaderFile = shader && join(a.results, 'shaders', shader[1]);
        if (shaderFile) return existsSync(shaderFile) ? send(response, 200, 'text/plain', readFileSync(shaderFile)) : send(response, 404, 'text/plain', '');
        if (path === '/phone/cert.cer') return send(response, 200, 'application/x-x509-ca-cert', tls.cert);
        if (path === '/phone/timing.json') return send(response, 200, 'application/json', JSON.stringify({ url: requests ? `/plan-timing.html?requests=/@fs${requests}/&runs=3` : null }));
        // The timing page reads the requests and their fixture files by absolute path, as Vite's /@fs/ does.
        if (requests && path.startsWith('/@fs/')) {
            const real = path.slice(4), allowed = [requests, join(HERE, 'fixture')];
            return allowed.some(root => real.startsWith(`${root}/`)) && existsSync(real) ? send(response, 200, TYPES[extname(real)] ?? 'application/octet-stream', readFileSync(real)) : send(response, 404, 'text/plain', '');
        }
        const local = join(DIST, path);
        if (local.startsWith(DIST) && extname(local) && existsSync(local)) {
            let body = readFileSync(local);
            const type = TYPES[extname(local)] ?? 'application/octet-stream';
            // The ground worker's jobs carry their time split (phone-worker.js).
            if (/\/ground-worker-[\w-]+\.js$/.test(local)) body = Buffer.concat([readFileSync(join(HERE, 'phone-worker.js')), body]);
            // The scripts run before the page's module, so they see the WebGPU device as the page asks for it. Each acts only
            // while a session of its own runs in the tab.
            return send(response, 200, type, type === 'text/html' ? body.toString().replace('<head>', '<head><script src="/phone/measure.js"></script><script src="/phone/diagnose.js"></script>') : body);
        }
        proxy(request, response);
    }).listen(+a.port, a.host, () => console.log(`${origin}/phone/ · data ${a.data} · results ${a.results}`));
}

/** Passes a read to the data origin with the phone's Host, so the catalogue names this origin; its JSON then says https. */
function proxy(request, response) {
    const host = request.headers.host, target = new URL(request.url, a.data);
    const out = httpRequest(target, { method: request.method, headers: { ...request.headers, host, 'accept-encoding': 'identity' } }, answer => {
        const headers = { ...answer.headers, 'cache-control': 'no-store' };
        delete headers['access-control-allow-origin'];
        if (!String(answer.headers['content-type']).includes('json')) return response.writeHead(answer.statusCode, headers), answer.pipe(response);
        const parts = [];
        answer.on('data', part => parts.push(part)).on('end', () => {
            const body = Buffer.concat(parts).toString().replaceAll(`http://${host}`, `https://${host}`);
            delete headers['content-length'];
            response.writeHead(answer.statusCode, headers).end(body);
        });
    });
    out.on('error', error => response.writeHead(502, { 'content-type': 'text/plain' }).end(String(error)));
    request.pipe(out);
}

/** A POST of `{session, key, value}`: `event` appends, `runs.N` merges into that run, any other key replaces. */
function collect(request, response) {
    const parts = [];
    request.on('data', part => parts.push(part)).on('end', () => {
        try {
            const { session, key, value } = JSON.parse(Buffer.concat(parts).toString());
            if (!/^[\w-]{1,64}$/.test(session)) throw new Error('bad session');
            const path = join(a.results, `${session}.json`);
            const record = existsSync(path) ? JSON.parse(readFileSync(path, 'utf8')) : { session, received: new Date().toISOString(), runs: [], events: [] };
            const run = /^runs\.(\d+)$/.exec(key);
            if (key === 'event') record.events.push({ at: new Date().toISOString(), ...value });
            else if (run) record.runs[+run[1]] = { ...record.runs[+run[1]], ...value };
            else record[key] = value;
            writeFileSync(`${path}.tmp`, JSON.stringify(record, null, 1));
            renameSync(`${path}.tmp`, path);
            response.writeHead(204).end();
        } catch (error) {
            response.writeHead(400, { 'content-type': 'text/plain' }).end(String(error));
        }
    });
}

/** A POST of bytes to /blob?session=S&name=N: they replace RESULTS/S/N. */
function keep(request, response) {
    const query = new URL(request.url, 'https://phone').searchParams, session = query.get('session'), name = query.get('name');
    if (!/^[\w-]{1,64}$/.test(session ?? '') || !/^[\w.-]{1,160}$/.test(name ?? '')) return response.writeHead(400).end('bad session or name');
    const parts = [];
    request.on('data', part => parts.push(part)).on('end', () => {
        mkdirSync(join(a.results, session), { recursive: true });
        writeFileSync(join(a.results, session, name), Buffer.concat(parts));
        response.writeHead(204).end();
    });
}

/** The start and end page: the script of the mode draws it. */
const page = mode => `<!doctype html><html lang="en"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>TrailView phone test</title><script src="/phone/${mode}.js"></script></head><body></body></html>`;

/** One table of a session file: per run its load marks, its flight and its memory. */
function summarize(record) {
    const d = record.device ?? {};
    console.log(`session ${record.session} · ${d.userAgent ?? '?'}`);
    console.log(`screen ${d.screen?.join('×')} @${d.pixelRatio} · adapter ${d.adapter?.info ?? d.adapter?.error ?? '?'} · format ${d.format} · timestamp-query ${d.adapter?.features?.includes('timestamp-query') ?? '?'}`);
    const f = (value, digits = 1) => (typeof value === 'number' && Number.isFinite(value) ? value.toFixed(digits) : '–');
    const rows = [['run', 'variant', 'scene', 'canvas', 'first s', 'eye s', 'near s', 'proper s', 'frame med', 'p95', 'p99', 'max', '>16.7%', '>33.3%', 'cpu med', 'cpu p95', 'done med', 'done p95', 'gpu med', 'pools MiB peak/end', 'records heap', 'quality', 'finest', 'pieces drawn', 'problems']];
    for (const [i, run] of record.runs.entries()) {
        if (!run) continue;
        const m = run.marks ?? {}, s = run.flight?.summary ?? {}, mem = run.memory ?? {}, look = run.flight?.look ?? {};
        // Pages left out per level print as the HUD prints them: L18 3.
        const value = v => (Array.isArray(v) ? v.flatMap((n, level) => (n ? [`L${level} ${n}`] : [])).join(' ') : v);
        const problems = [run.failure, ...Object.entries(run.problems ?? {}).filter(([, v]) => value(v)).map(([k, v]) => `${k} ${value(v)}`)].filter(Boolean).join('; ');
        rows.push([i + 1, run.variant, run.scene, run.canvas?.join('×') ?? '–', f(m.firstFrame / 1000), f(m.eyeReady / 1000), f(m.nearReady / 1000), f(m.proper / 1000),
            f(s.interval?.median), f(s.interval?.p95), f(s.interval?.p99), f(s.interval?.max), f(s.over16, 0), f(s.over33, 0), f(s.cpu?.median, 2), f(s.cpu?.p95, 2),
            f(s.done?.median), f(s.done?.p95), f(s.gpu?.median, 2), `${f(mem.poolsPeak / 2 ** 20, 0)}/${f(mem.poolsEnd / 2 ** 20, 0)}`, f(mem.planHeap / 2 ** 20, 0),
            look.quality ? look.quality.join('–') : '–', look.finest ? look.finest.join('–') : '–', look.piecesDrawn ? look.piecesDrawn.join('–') : '–', problems || '–']);
    }
    const widths = rows[0].map((_, c) => Math.max(...rows.map(row => String(row[c]).length)));
    for (const row of rows) console.log(row.map((cell, c) => String(cell).padEnd(widths[c])).join(' '));
    const t = record.timing;
    if (t) console.log(t.error ? `plan timing: ${t.error}` : `plan timing: ${t.cells?.length} cells · median ${f(t.summary?.median)} ms · p95 ${f(t.summary?.p95)} ms · prepare ${f(t.summary?.prepare)} ms · heap ${f(t.summary?.heap / 2 ** 20, 0)} MiB · ${t.summary?.same}/${t.summary?.of} digests equal`);
    // The ground worker's jobs of each run's load: count, round trip median, p95 and sum in ms, and the probe to the last page.
    for (const [i, run] of record.runs.entries()) {
        const g = run?.ground;
        if (g) console.log(`ground run ${i + 1}: load ${g.load.jobs} jobs (${g.load.failed ?? 0} failed) · median ${f(g.load.median)} p95 ${f(g.load.p95)} sum ${f(g.load.sum, 0)} ms · probe to last page ${f(g.load.openToLast / 1000, 2)} s · flight ${g.flight.jobs} jobs, median ${f(g.flight.median)} ms`);
    }
    const l = record.groundLoad;
    if (l) console.log(`ground load (diagnose): ${l.jobs} jobs · median ${f(l.median)} p95 ${f(l.p95)} sum ${f(l.sum, 0)} ms · probe to last page ${f(l.openToLast / 1000, 2)} s`);
    for (const [key, value] of Object.entries(record)) if (key.startsWith('cold.')) coldStep(record, key.slice(5), value, f);
    // The fault check's events: only the errors.
    const shown = record.events.filter(e => e.text || (/error|lost|rejected|failed/.test(e.kind) && !(e.kind === 'first frames scope' && !e.found)));
    for (const event of shown) console.log(`event ${event.at} ${event.run ?? event.step ?? '–'}: ${event.text ?? `${event.kind} ${JSON.stringify({ ...event, at: undefined, step: undefined, kind: undefined }).slice(0, 300)}`}`);
}

/** One load of the cold start check: its marks, the ground jobs' time split, the requests, the drawn pages' spikes, and a line per picture. */
function coldStep(record, step, c, f) {
    const s = ms => f(ms / 1000), m = c.marks ?? {}, median = list => [...list].sort((a, b) => a - b)[list.length >> 1];
    console.log(`\n${step}: eye ${s(m.eyeReady)} s · near ${s(m.nearReady)} s · proper ${s(m.proper)} s · flight from ${s(c.flightStart)} s · frame median ${f(c.frame?.median, 0)} p95 ${f(c.frame?.p95, 0)} max ${f(c.frame?.max, 0)} ms`);
    for (const [phase, jobs] of [['load', (c.ground ?? []).filter(j => j.start < c.flightStart)], ['flight', (c.ground ?? []).filter(j => j.start >= c.flightStart)]]) {
        if (!jobs.length) continue;
        const of = k => f(median(jobs.map(j => j[k]).filter(v => v !== undefined)), 0);
        console.log(`  ground ${phase}: ${jobs.length} jobs, median ms: round trip ${of('ms')} = wait ${of('wait')} + worker ${of('work')} (decode ${of('decode')}, build ${of('build')}) + back ${of('back')} · failed ${jobs.filter(j => j.failed).length}`);
    }
    for (const [group, g] of Object.entries(c.fetches ?? {})) console.log(`  requests ${group}: ${g.count} · median ${f(g.median, 0)} p95 ${f(g.p95, 0)} ms · ${f(g.bytes / 2 ** 20, 1)} MiB`);
    for (const when of ['ready', 'end']) {
        const p = record[`pages.${step}-${when}`];
        if (!p) continue;
        // Per level: the largest 4-neighbour Laplacian of a drawn page, in metres; a height that does not belong to the land shows here.
        const levels = {};
        for (const r of p.rows) levels[r.name.split('/')[0]] = Math.max(levels[r.name.split('/')[0]] ?? 0, r.maxLaplacian);
        console.log(`  pages ${when}: ${p.count} drawn, ${p.rows.filter(r => !r.complete).length} not complete · max Laplacian m per level: ${Object.entries(levels).map(([l, v]) => `L${l} ${f(v, 0)}`).join(' ')}`);
    }
    console.log('  picture                     s     phase  t     frame nodes own  loads miss stamps n/c/m ground short errors');
    for (const p of c.shots ?? []) {
        console.log(`  ${p.name.padEnd(27)} ${s(p.at).padEnd(5)} ${p.phase.padEnd(6)} ${String(p.t ?? '').padEnd(5)} ${String(p.frame ?? '').padEnd(5)} ${String(p.nodes).padEnd(5)} ${String(p.own).padEnd(4)} ${String(p.loads).padEnd(5)} ${String(p.missing).padEnd(4)} ${`${p.stamps?.near}/${p.stamps?.coarse}/${p.stamps?.masks}`.padEnd(12)} ${String(p.ground).padEnd(6)} ${String(p.groundShort).padEnd(5)} ${p.errors}${p.blank ? ' (no new frame: no picture)' : ''}`);
    }
}
