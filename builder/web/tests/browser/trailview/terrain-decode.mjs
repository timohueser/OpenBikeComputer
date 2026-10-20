// The time that a worker takes to decode one 512 px terrain tile into metres, as TrailView does:
// createImageBitmap without colour conversion, then the pixels, then R × 256 + G + B / 256 − 32768.
//
//   node trailview/terrain-decode.mjs TILE.webp|URL [--runs N] [--channel chrome]
//
// Run it from builder/web/tests/browser on the machine that is measured. It prints the median, the
// 95th percentile and the maximum of N decodes in milliseconds. The first decode is not counted.
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { chromium } from 'playwright';

const args = process.argv.slice(2);
const option = (name, fallback) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
const source = args[0];
if (!source || source.startsWith('--')) throw new Error('Give a terrain tile: a .webp file or its URL');
const runs = Number(option('--runs', 200));
const tile = Buffer.from(source.startsWith('http') ? await (await fetch(source)).arrayBuffer() : await readFile(source));

const worker = `
onmessage = async ({ data: { tile, runs } }) => {
    const blob = new Blob([tile], { type: 'image/webp' });
    const times = [];
    let known = 0;
    for (let run = 0; run <= runs; run++) {
        const start = performance.now();
        const bitmap = await createImageBitmap(blob, { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
        const context = new OffscreenCanvas(bitmap.width, bitmap.height).getContext('2d', { willReadFrequently: true });
        context.drawImage(bitmap, 0, 0);
        const rgba = context.getImageData(0, 0, bitmap.width, bitmap.height).data;
        bitmap.close();
        const metres = new Float32Array(rgba.length / 4);
        for (let i = 0; i < metres.length; i++) {
            metres[i] = rgba[4 * i + 3] === 255 ? rgba[4 * i] * 256 + rgba[4 * i + 1] + rgba[4 * i + 2] / 256 - 32768 : NaN;
        }
        if (run > 0) times.push(performance.now() - start);
        known = metres.filter(Number.isFinite).length;
    }
    times.sort((a, b) => a - b);
    const at = q => Number(times[Math.min(times.length - 1, Math.floor(q * times.length))].toFixed(2));
    postMessage({ runs, known, median_ms: at(0.5), p95_ms: at(0.95), max_ms: Number(times.at(-1).toFixed(2)) });
};`;
const page = `<!doctype html><script type="module">
const tile = await (await fetch('/tile.webp')).arrayBuffer();
const worker = new Worker(URL.createObjectURL(new Blob([${JSON.stringify(worker)}], { type: 'text/javascript' })));
worker.onmessage = ({ data }) => { window.result = data; };
worker.postMessage({ tile, runs: ${runs} }, [tile]);
</script>`;

const server = createServer((request, response) => {
    const tiled = request.url === '/tile.webp';
    response.writeHead(200, { 'content-type': tiled ? 'image/webp' : 'text/html' });
    response.end(tiled ? tile : page);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ channel: option('--channel', undefined) });
try {
    const tab = await browser.newPage();
    await tab.goto(`http://127.0.0.1:${server.address().port}/`);
    const result = await tab.waitForFunction(() => window.result, null, { timeout: 600_000 });
    console.log(JSON.stringify({ tile: source, bytes: tile.length, browser: browser.version(), ...(await result.jsonValue()) }));
} finally {
    await browser.close();
    server.close();
}
