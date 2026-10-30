import { existsSync, openSync, readFileSync, readSync } from 'node:fs';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';
import { chromium } from 'playwright';
import { PMTiles } from 'pmtiles';

/** WebGPU on the CPU: SwiftShader draws the same picture as a GPU, slowly. Its times say nothing about a GPU. */
export const SWIFTSHADER = ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=swiftshader', '--use-angle=swiftshader', '--use-webgpu-adapter=swiftshader'];
/** A Chromium other than the one Playwright installed, such as /opt/pw-browsers/chromium. */
export const EXECUTABLE = process.env.OBC_CHROMIUM || undefined;
/** WebGPU on the machine's GPU. Chromium on Linux gives no adapter without Vulkan. */
const GPU = ['--enable-webgpu-developer-features', ...(process.platform === 'linux' ? ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-angle=vulkan', '--ignore-gpu-blocklist'] : [])];
/**
 * The browser of a script: with `gpu` the machine's GPU, else SwiftShader. Only a GPU `window` is not
 * headless, and macOS keeps one by default: its GPU is proven in a window only. With OBC_BROWSER_WS set,
 * it connects to that Playwright browser server instead; closing it then only disconnects.
 */
export function launch(gpu, window = process.platform !== 'linux') {
    if (process.env.OBC_BROWSER_WS) return chromium.connect(process.env.OBC_BROWSER_WS);
    return chromium.launch({ executablePath: EXECUTABLE, headless: !(gpu && window), args: gpu ? GPU : SWIFTSHADER });
}
export const DIST = new URL('../../../dist/trailview/', import.meta.url).pathname;
const FIXTURE = new URL('./fixture/terrain/', import.meta.url).pathname;
const FEATURES = new URL('./fixture/features/', import.meta.url).pathname;
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.txt': 'text/plain', '.svg': 'image/svg+xml' };
/** `ground` of routeFixture: one ground everywhere, so the fixture has ground pages: the first material, lithology and region of FLAT_RULES. */
export const FLAT = 'flat';
/** A 512 px lossless WebP ground tile of zero bytes (cwebp -lossless -exact). */
const FLAT_TILE = Buffer.from('UklGRiwAAABXRUJQVlA4TB8AAAAv/8F/EAcQEREQCCT7e89QRP8z/vOf//znP//5z/8BAA==', 'base64');
/** The `world` region of the engine's ground without a ruleset (ground.ts), as a rules.json. */
const FLAT_RULES = {
    format: 3, materials: { materials: [{ id: 'none', cover: 'grass' }], lithologies: [{ id: 'unknown', hardness: 0.5 }] },
    ids: [['world', '*']],
    regions: {
        world: { leaf: 'mixed', look: {
            snap: 0.4, hazeKm: 35, hazeHeight: 2500, mistTop: 0, softness: 0.08, gradient: 0.35, ramps: {}, outcrops: 0, snow_share: 0.5, turf_deg: 42,
            rock: '#8c877d', sea: { deep: '#2c5874', shallow: '#4f8088' }, grade: { shadow: [-0.004, 0, 0.012], light: [0.01, 0.004, -0.008], saturation: 1.08 },
        } },
    },
};

/**
 * The tile service of the planner hosts at `origin`: a function from a path to its answer
 * `{ status, type, body }`, or undefined for a path that the service does not know. The terrain tiles
 * come from the fixture, or from `tiles`, a `planner-dem` MBTiles, and 204 answers an absent tile as
 * the tile service does. With `ground`, a folder of the TrailView bakes, it answers their TileJSON and
 * tiles, and `trailview/rules.json`; `colour` names the folder of the colour variant in it. The feature
 * tiles come from the fixture, or from `features`, a folder of `z/x/y.bin`. `ground` may be FLAT.
 */
export async function tileService(origin, tiles, ground, colour = 'webp-12', features = FEATURES) {
    const read = tiles?.endsWith('.mbtiles') ? await mbtiles(tiles) : (z, x, y) => {
        const path = join(tiles ?? FIXTURE, `${z}/${x}/${y}.webp`);
        return existsSync(path) ? readFileSync(path) : undefined;
    };
    const layers = ground === FLAT ? { 'trailview-ground': FLAT_ARCHIVE } : ground ? groundLayers(ground, colour) : {};
    const none = { status: 204, body: Buffer.alloc(0) };
    const found = (body, type) => (body ? { status: 200, type, body } : none);
    const json = value => found(Buffer.from(JSON.stringify(value)), 'application/json');
    return async path => {
        if (path === '/trailview-features.json') {
            return json({ tilejson: '3.0.0', tiles: [`${origin}/trailview-features/{z}/{x}/{y}.bin`], minzoom: 8, maxzoom: 13, bounds: [-180, -85, 180, 85], features_format: 2, extent: 16384 });
        }
        const feature = /^\/trailview-features\/(\d+\/\d+\/\d+\.bin)$/.exec(path);
        if (feature) {
            const file = join(features, feature[1]);
            return found(existsSync(file) && readFileSync(file), 'application/octet-stream');
        }
        const tile = /^\/terrain\/(\d+)\/(\d+)\/(\d+)\.webp$/.exec(path);
        if (tile) return found(read(...tile.slice(1).map(Number)), 'image/webp');
        if (ground && path === '/trailview/rules.json') return ground === FLAT ? json(FLAT_RULES) : found(readFileSync(join(ground, 'rules.json')), 'application/json');
        const [, name, z, x, y] = /^\/([a-z0-9-]+)(?:\.json|\/(\d+)\/(\d+)\/(\d+))$/.exec(path) ?? [];
        const archive = layers[name];
        if (!archive) return undefined;
        if (z === undefined) {
            const [header, metadata] = await Promise.all([archive.getHeader(), archive.getMetadata()]);
            return json({ ...metadata, minzoom: header.minZoom, maxzoom: header.maxZoom, bounds: [header.minLon, header.minLat, header.maxLon, header.maxLat], tiles: [`${origin}/${name}/{z}/{x}/{y}`] });
        }
        const data = (await archive.getZxy(Number(z), Number(x), Number(y)))?.data;
        return found(data && Buffer.from(data), 'application/octet-stream');
    };
}

/** Answers the planner hosts of `fixture/config.json` in the page with tileService; every other public host is refused. */
export async function routeFixture(page, tiles, ground, colour, features) {
    const answer = await tileService('https://tiles.test', tiles, ground, colour, features);
    await page.route(url => url.hostname !== '127.0.0.1', async route => {
        const url = new URL(route.request().url());
        const found = url.hostname === 'tiles.test' && await answer(url.pathname);
        if (!found) return route.abort();
        return route.fulfill({ status: found.status, body: found.body, contentType: found.type, headers: { 'access-control-allow-origin': '*' } });
    });
}

/** The ground archive of FLAT: every tile of zooms 0 to 13 is FLAT_TILE. */
const FLAT_ARCHIVE = {
    getHeader: async () => ({ minZoom: 0, maxZoom: 13, minLon: -180, minLat: -85, maxLon: 180, maxLat: 85 }),
    getMetadata: async () => ({ trailview_format: 3 }),
    getZxy: async () => ({ data: FLAT_TILE }),
};

/** The archives of the TrailView bakes in a folder, by their names in the catalogue's `layers`. */
function groundLayers(dir, colour) {
    const files = {
        'trailview-ground': 'ground.pmtiles', 'trailview-canopy': 'trailview-canopy.pmtiles',
        ...Object.fromEntries(['q1', 'q2', 'q3', 'q4'].map(name => [`trailview-colour-${name}`, `colour/${colour}/${name}.pmtiles`])),
        ...Object.fromEntries(Array.from({ length: 46 }, (_, band) => String(band).padStart(2, '0')).map(band => [`trailview-snow-${band}`, `snow-days/${band}.pmtiles`])),
    };
    return Object.fromEntries(Object.entries(files).filter(([, file]) => existsSync(join(dir, file))).map(([name, file]) => [name, new PMTiles(new FileSource(join(dir, file)))]));
}

/** A PMTiles source over a local file. */
class FileSource {
    constructor(path) { this.path = path; this.fd = openSync(path, 'r'); }
    getKey() { return this.path; }
    async getBytes(offset, length) {
        const data = Buffer.alloc(length);
        const read = readSync(this.fd, data, 0, length, offset);
        return { data: data.buffer.slice(data.byteOffset, data.byteOffset + read) };
    }
}

/** The tiles of an MBTiles file, which numbers rows from the south. */
async function mbtiles(file) {
    const { DatabaseSync } = await import('node:sqlite');
    const query = new DatabaseSync(file, { readOnly: true }).prepare('SELECT tile_data FROM tiles WHERE zoom_level = ? AND tile_column = ? AND tile_row = ?');
    return (z, x, y) => {
        const row = query.get(z, x, 2 ** z - 1 - y);
        return row && Buffer.from(row.tile_data);
    };
}

/** The pixels of a screenshot of the page that show the red clear colour of `debug=cracks`. */
export async function redPixels(page) {
    const png = (await page.screenshot()).toString('base64');
    return page.evaluate(async png => {
        const image = await createImageBitmap(await (await fetch(`data:image/png;base64,${png}`)).blob());
        const context = new OffscreenCanvas(image.width, image.height).getContext('2d');
        context.drawImage(image, 0, 0);
        const rgba = context.getImageData(0, 0, image.width, image.height).data;
        let red = 0;
        for (let i = 0; i < rgba.length; i += 4) if (rgba[i] === 255 && rgba[i + 1] === 0 && rgba[i + 2] === 0) red++;
        return red;
    }, png);
}

/** Serves the built page on a free loopback port; resolves to its origin and a close function. */
export async function serve(root = DIST) {
    const server = createServer((request, response) => {
        const path = join(root, normalize(new URL(request.url, 'http://x').pathname));
        if (!path.startsWith(root) || !existsSync(path) || !extname(path)) return response.writeHead(404).end();
        response.writeHead(200, { 'content-type': TYPES[extname(path)] ?? 'application/octet-stream' }).end(readFileSync(path));
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    return { origin: `http://127.0.0.1:${server.address().port}`, close: () => server.close() };
}

/**
 * Collects page errors and console errors, where the engine reports WebGPU validation errors. A
 * failed resource is left out: the site's icon is absent here, and the engine counts failed tiles.
 */
export function errorsOf(page) {
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => message.type() === 'error' && !message.text().startsWith('Failed to load resource') && errors.push(message.text()));
    return errors;
}
