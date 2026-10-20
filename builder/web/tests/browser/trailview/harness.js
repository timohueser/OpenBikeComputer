import { existsSync, readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';

/** WebGPU on the CPU: SwiftShader draws the same picture as a GPU, slowly. Its times say nothing about a GPU. */
export const SWIFTSHADER = ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=swiftshader', '--use-angle=swiftshader', '--use-webgpu-adapter=swiftshader'];
/** A Chromium other than the one Playwright installed, such as /opt/pw-browsers/chromium. */
export const EXECUTABLE = process.env.OBC_CHROMIUM || undefined;
export const DIST = new URL('../../../dist/trailview/', import.meta.url).pathname;
const FIXTURE = new URL('./fixture/terrain/', import.meta.url).pathname;
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.txt': 'text/plain', '.svg': 'image/svg+xml' };

/**
 * Answers the planner hosts of `fixture/config.json` in the page: the terrain tiles from the
 * fixture, 204 for an absent tile like the tile service. Every other public host is refused.
 */
export async function routeFixture(page) {
    await page.route(url => url.hostname !== '127.0.0.1', route => {
        const url = new URL(route.request().url());
        const tile = /^\/terrain\/(\d+\/\d+\/\d+\.webp)$/.exec(url.pathname);
        if (url.hostname !== 'tiles.test' || !tile) return route.abort();
        const path = join(FIXTURE, tile[1]);
        if (!existsSync(path)) return route.fulfill({ status: 204, headers: { 'access-control-allow-origin': '*' } });
        return route.fulfill({ body: readFileSync(path), contentType: 'image/webp', headers: { 'access-control-allow-origin': '*' } });
    });
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
