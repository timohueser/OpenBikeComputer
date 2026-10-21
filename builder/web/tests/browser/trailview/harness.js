import { existsSync, readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';

/** WebGPU on the CPU: SwiftShader draws the same picture as a GPU, slowly. Its times say nothing about a GPU. */
export const SWIFTSHADER = ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=swiftshader', '--use-angle=swiftshader', '--use-webgpu-adapter=swiftshader'];
/** A Chromium other than the one Playwright installed, such as /opt/pw-browsers/chromium. */
export const EXECUTABLE = process.env.OBC_CHROMIUM || undefined;
export const DIST = new URL('../../../dist/trailview/', import.meta.url).pathname;
const FIXTURE = new URL('./fixture/terrain/', import.meta.url).pathname;
const FEATURES = new URL('./fixture/features/', import.meta.url).pathname;
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.txt': 'text/plain', '.svg': 'image/svg+xml' };

/**
 * Answers the planner hosts of `fixture/config.json` in the page: the terrain tiles from the fixture,
 * or from `tiles`, a `planner-dem` MBTiles; the feature tiles from the fixture, or from `features`, a
 * folder of `z/x/y.bin`; and 204 for an absent tile like the tile service. Every other public host is
 * refused.
 */
export async function routeFixture(page, tiles, features = FEATURES) {
    const read = tiles?.endsWith('.mbtiles') ? await mbtiles(tiles) : (z, x, y) => {
        const path = join(tiles ?? FIXTURE, `${z}/${x}/${y}.webp`);
        return existsSync(path) ? readFileSync(path) : undefined;
    };
    await page.route(url => url.hostname !== '127.0.0.1', route => {
        const url = new URL(route.request().url());
        const cors = { 'access-control-allow-origin': '*' };
        if (url.hostname === 'tiles.test' && url.pathname === '/trailview-features.json') {
            const tilejson = { tilejson: '3.0.0', tiles: ['https://tiles.test/trailview-features/{z}/{x}/{y}.bin'], minzoom: 8, maxzoom: 14, bounds: [-180, -85, 180, 85] };
            return route.fulfill({ body: JSON.stringify(tilejson), contentType: 'application/json', headers: cors });
        }
        const feature = /^\/trailview-features\/(\d+\/\d+\/\d+\.bin)$/.exec(url.pathname);
        if (url.hostname === 'tiles.test' && feature) {
            const path = join(features, feature[1]);
            if (!existsSync(path)) return route.fulfill({ status: 204, headers: cors });
            return route.fulfill({ body: readFileSync(path), contentType: 'application/octet-stream', headers: cors });
        }
        const tile = /^\/terrain\/(\d+)\/(\d+)\/(\d+)\.webp$/.exec(url.pathname);
        if (url.hostname !== 'tiles.test' || !tile) return route.abort();
        const body = read(...tile.slice(1).map(Number));
        if (!body) return route.fulfill({ status: 204, headers: { 'access-control-allow-origin': '*' } });
        return route.fulfill({ body, contentType: 'image/webp', headers: { 'access-control-allow-origin': '*' } });
    });
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
