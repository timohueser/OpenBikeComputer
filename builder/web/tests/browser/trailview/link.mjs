// The page and the tile service on two HTTP/2 origins behind one shaped link, and the log of what
// crossed it, for network.mjs. Tiles go out as planner/tiles/src/worker.mjs sends them: stored bytes,
// gzip feature tiles with Content-Encoding gzip, Cache-Control immutable. Text goes out as brotli.
//
// The link: a response waits one round trip (three on the first stream of a connection: TCP and
// TLS 1.3), then its body shares the bandwidth with the other bodies, round robin in 8 kB chunks.
import { execFileSync } from 'node:child_process';
import { createHash, createPublicKey } from 'node:crypto';
import { existsSync, mkdtempSync, openSync, readFileSync, readSync, readdirSync, rmSync } from 'node:fs';
import { createSecureServer } from 'node:http2';
import { tmpdir } from 'node:os';
import { extname, join, normalize } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { brotliCompressSync, constants } from 'node:zlib';
import { PMTiles } from 'pmtiles';

const CHUNK = 8192;
const TEXT = { '.html': 'text/html', '.js': 'text/javascript', '.json': 'application/json', '.svg': 'image/svg+xml', '.txt': 'text/plain' };
const IMMUTABLE = { 'cache-control': 'public, max-age=31536000, immutable' };
const now = () => performance.timeOrigin + performance.now();

class FileSource {
    constructor(path) { this.path = path; this.fd = openSync(path, 'r'); }
    getKey() { return this.path; }
    async getBytes(offset, length) {
        const data = Buffer.alloc(length), read = readSync(this.fd, data, 0, length, offset);
        return { data: data.buffer.slice(data.byteOffset, data.byteOffset + read) };
    }
}

/** The archives of a ground folder (the layout of `--ground` in README.md) by their names in the config's `layers`. */
export function groundArchives(ground, colour) {
    const files = {
        'trailview-ground': 'ground.pmtiles', 'trailview-canopy': 'trailview-canopy.pmtiles',
        ...Object.fromEntries([1, 2, 3, 4].map(q => [`trailview-colour-q${q}`, `colour/${colour}/q${q}.pmtiles`])),
        ...Object.fromEntries(Array.from({ length: 46 }, (_, b) => [`trailview-snow-${String(b).padStart(2, '0')}`, `snow-days/${String(b).padStart(2, '0')}.pmtiles`])),
    };
    return Object.fromEntries(Object.entries(files).filter(([, f]) => existsSync(join(ground, f))).map(([name, f]) => [name, new PMTiles(new FileSource(join(ground, f)))]));
}

/**
 * Builds the TrailView page of a checkout's `builder/web` into `out` against the tile origin `tiles`,
 * with the layers of groundArchives in a box of `bounds`.
 */
export function buildPage({ web, tiles, bounds, out }) {
    const config = JSON.parse(readFileSync(new URL('./fixture/config.json', import.meta.url), 'utf8'));
    const names = ['trailview-features', 'trailview-ground', 'trailview-canopy', ...[1, 2, 3, 4].map(q => `trailview-colour-q${q}`),
        ...Array.from({ length: 46 }, (_, b) => `trailview-snow-${String(b).padStart(2, '0')}`)];
    Object.assign(config, {
        name: 'TrailView link', bounds, terrain: `${tiles}/terrain/{z}/{x}/{y}`, trailview_rules: `${tiles}/trailview/rules.json`,
        layers: Object.fromEntries(names.map(name => [name, `${tiles}/${name}.json`])),
    });
    execFileSync('npx', ['vite', 'build', '--mode', 'trailview', '--outDir', out, '--emptyOutDir', '--logLevel', 'error'],
        { cwd: web, env: { ...process.env, VITE_PLANNER_CONFIG: JSON.stringify(config) }, stdio: 'inherit' });
}

/** A self-signed certificate for 127.0.0.1 and the hash that Chromium's --ignore-certificate-errors-spki-list takes. */
function certificate() {
    const dir = mkdtempSync(join(tmpdir(), 'trailview-link-'));
    try {
        execFileSync('openssl', ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes', '-days', '2', '-subj', '/CN=127.0.0.1',
            '-addext', 'subjectAltName=IP:127.0.0.1', '-keyout', join(dir, 'key.pem'), '-out', join(dir, 'cert.pem')], { stdio: 'ignore' });
        const cert = readFileSync(join(dir, 'cert.pem')), key = readFileSync(join(dir, 'key.pem'));
        const spki = createHash('sha256').update(createPublicKey(cert).export({ type: 'spki', format: 'der' })).digest('base64');
        return { cert, key, spki };
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
}

/**
 * Starts both origins on `port` (the page) and `port + 1` (the tiles) at `mbit` Mbit/s and `rtt` ms.
 * `dist` is the built page, `terrain` an MBTiles, `ground` a ground folder, `features` a folder
 * `z/x/y.bin`. Returns the origins, the Chromium flag for the certificate, the log and close().
 */
export async function startLink({ dist, terrain, ground, colour = 'webp-12', features, mbit, rtt, port = 8951 }) {
    const rate = mbit * 1e6 / 8 / 1000; // bytes per ms
    const tiles = `https://127.0.0.1:${port + 1}`;
    const log = [];
    const tileRow = new DatabaseSync(terrain, { readOnly: true }).prepare('SELECT tile_data FROM tiles WHERE zoom_level = ? AND tile_column = ? AND tile_row = ?');
    const layers = groundArchives(ground, colour);
    const packed = new Map();
    const text = (key, body, type, accept) => {
        if (!/\bbr\b/.test(accept)) return { status: 200, headers: { 'content-type': type }, body };
        if (!packed.has(key)) packed.set(key, brotliCompressSync(body, { params: { [constants.BROTLI_PARAM_QUALITY]: 11 } }));
        return { status: 200, headers: { 'content-type': type, 'content-encoding': 'br' }, body: packed.get(key) };
    };

    // The bottleneck: one queue of bodies, round robin.
    const active = [];
    let inflight = null, busyUntil = 0, timer = null;
    function pump() {
        timer = null;
        for (;;) {
            const t = performance.now();
            if (inflight && busyUntil <= t) {
                const { job, chunk } = inflight;
                inflight = null;
                if (!job.stream.destroyed && !job.stream.closed) job.stream.write(chunk);
                job.sent += chunk.length;
                if (job.sent >= job.body.length) job.finish();
                else active.push(job);
            }
            if (!inflight) {
                let job;
                while ((job = active.shift()) && (job.stream.destroyed || job.stream.closed)) job.cancel();
                if (!job) break;
                const chunk = job.body.subarray(job.sent, job.sent + CHUNK);
                busyUntil = Math.max(busyUntil, t - 5) + chunk.length / rate;
                inflight = { job, chunk };
            }
            if (busyUntil > performance.now()) break;
        }
        if (inflight) timer = setTimeout(pump, Math.max(0, busyUntil - performance.now()));
    }

    function respond(stream, headers, origin) {
        const entry = { t0: now(), origin, path: headers[':path'], status: 0, bytes: 0 };
        log.push(entry);
        const session = stream.session, trips = session.used ? 1 : 3;
        session.used = true;
        const accept = String(headers['accept-encoding'] ?? '');
        Promise.resolve(origin === 'page' ? page(entry.path, accept) : tile(entry.path, accept))
            .catch(error => ({ status: 500, headers: {}, body: Buffer.from(String(error)) }))
            .then(answer => setTimeout(() => {
                if (stream.destroyed || stream.closed) return;
                stream.respond({ ':status': answer.status, 'access-control-allow-origin': '*', ...answer.headers, ...(answer.body.length ? { 'content-length': answer.body.length } : {}) });
                entry.status = answer.status;
                if (!answer.body.length) {
                    stream.end();
                    entry.te = now();
                    return;
                }
                const job = {
                    stream, body: answer.body, sent: 0,
                    finish() { stream.end(); entry.bytes = job.sent; entry.te = now(); },
                    cancel() { entry.bytes = job.sent; entry.te = now(); },
                };
                active.push(job);
                if (!timer) pump();
            }, Math.max(0, entry.t0 + trips * rtt - now())));
    }

    function page(path, accept) {
        const file = join(dist, normalize(decodeURIComponent(new URL(path, 'https://x').pathname)));
        if (!file.startsWith(dist) || !existsSync(file) || !extname(file)) return { status: 404, headers: {}, body: Buffer.alloc(0) };
        const type = TEXT[extname(file)], body = readFileSync(file);
        const answer = type ? text(file, body, type, accept) : { status: 200, headers: { 'content-type': 'application/octet-stream' }, body };
        answer.headers['cache-control'] = file.includes('/assets/') ? IMMUTABLE['cache-control'] : 'no-cache';
        return answer;
    }

    const none = { status: 204, headers: IMMUTABLE, body: Buffer.alloc(0) };
    const json = (path, value, accept) => { const answer = text(path, Buffer.from(JSON.stringify(value)), 'application/json', accept); return { ...answer, headers: { ...IMMUTABLE, ...answer.headers } }; };
    async function tile(path, accept) {
        let m;
        if ((m = /^\/terrain\/(\d+)\/(\d+)\/(\d+)$/.exec(path))) {
            const [z, x, y] = m.slice(1).map(Number), row = tileRow.get(z, x, 2 ** z - 1 - y);
            return row ? { status: 200, headers: { ...IMMUTABLE, 'content-type': 'image/webp' }, body: Buffer.from(row.tile_data) } : none;
        }
        if (path === '/trailview/rules.json') return json(path, JSON.parse(readFileSync(join(ground, 'rules.json'), 'utf8')), accept);
        if (path === '/trailview-features.json') {
            return json(path, { tilejson: '3.0.0', tiles: [`${tiles}/trailview-features/{z}/{x}/{y}`], minzoom: 8, maxzoom: 13, bounds: [-180, -85, 180, 85], features_format: 2, extent: 16384 }, accept);
        }
        if ((m = /^\/trailview-features\/(\d+)\/(\d+)\/(\d+)$/.exec(path))) {
            const file = join(features, `${m[1]}/${m[2]}/${m[3]}.bin`);
            if (!existsSync(file)) return none;
            const body = readFileSync(file), gzip = body[0] === 0x1f && body[1] === 0x8b;
            return { status: 200, headers: { ...IMMUTABLE, 'content-type': 'application/octet-stream', ...(gzip ? { 'content-encoding': 'gzip' } : {}) }, body };
        }
        const [, name, z, x, y] = /^\/([a-z0-9-]+)(?:\.json|\/(\d+)\/(\d+)\/(\d+))$/.exec(path) ?? [];
        const archive = layers[name];
        if (!archive) return { status: 404, headers: {}, body: Buffer.alloc(0) };
        const header = await archive.getHeader();
        if (z === undefined) {
            const metadata = await archive.getMetadata();
            return json(path, { ...metadata, minzoom: header.minZoom, maxzoom: header.maxZoom, bounds: [header.minLon, header.minLat, header.maxLon, header.maxLat], tiles: [`${tiles}/${name}/{z}/{x}/{y}`] }, accept);
        }
        const found = await archive.getZxy(Number(z), Number(x), Number(y));
        if (!found) return none;
        return { status: 200, headers: { ...IMMUTABLE, 'content-type': header.tileType === 4 ? 'image/webp' : 'application/octet-stream' }, body: Buffer.from(found.data) };
    }

    // Packed before the first request: the brotli of the bundle takes most of a second.
    const walk = dir => readdirSync(dir, { withFileTypes: true }).flatMap(d => (d.isDirectory() ? walk(join(dir, d.name)) : [join(dir, d.name)]));
    for (const file of walk(dist)) if (TEXT[extname(file)]) text(file, readFileSync(file), TEXT[extname(file)], 'br');
    const { cert, key, spki } = certificate();
    const sessions = new Set();
    const servers = await Promise.all(['page', 'tiles'].map((origin, i) => new Promise(resolve => {
        const server = createSecureServer({ cert, key, settings: { maxConcurrentStreams: 256 } });
        server.on('session', session => { sessions.add(session); session.on('close', () => sessions.delete(session)); });
        server.on('stream', (stream, headers) => {
            stream.on('error', () => {});
            respond(stream, headers, origin);
        });
        server.listen(port + i, '127.0.0.1', () => resolve(server));
    })));
    return {
        page: `https://127.0.0.1:${port}`, tiles, log,
        flag: `--ignore-certificate-errors-spki-list=${spki}`,
        close: () => {
            for (const session of sessions) session.destroy();
            return Promise.all(servers.map(server => new Promise(resolve => server.close(resolve))));
        },
    };
}
