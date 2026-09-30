import * as maplibre from 'maplibre-gl';
import { Protocol } from 'pmtiles';
import contour from 'maplibre-contour';
import { mapStyle } from '../../../../../builder/app/src/lib/planner/map-style';
import { mapIcon } from '../../../../../builder/app/src/lib/planner/map-icons';
import { RouteOverlays } from '../../../../../builder/app/src/lib/planner/route-overlays';
import { terrainSource } from '../../../../../builder/app/src/lib/planner/map-terrain';

const report: any = { samples: [], errors: [], external_requests: [], workload: 'Production RouteOverlays, fixed390x763 viewport, actual API geometry/properties; zero-duration pan; idle includes symbol fade' };
const send = (kind: string, data: unknown) => {
    (window as any).webkit?.messageHandlers.mapBenchmark.postMessage({ kind, data });
    if (kind === 'progress') document.title = String(data);
    if (kind === 'complete') {
        if (!(window as any).webkit) void fetch('/report', { method: 'POST', body: JSON.stringify(data) });
        if (!report.persistent_map) {
            const output = document.createElement('pre'); output.textContent = JSON.stringify(data, null, 2);
            document.body.replaceChildren(output); document.body.style.overflow = 'auto';
        }
        document.title = 'Overlay benchmark complete';
    }
};
const absolute = (path: string) => new URL(path, location.href).href.replaceAll('%7B', '{').replaceAll('%7D', '}');
const protocol = new Protocol();
maplibre.addProtocol('pmtiles', protocol.tile);
maplibre.setWorkerUrl(absolute('map-worker.js'));
contour.workerUrl = absolute('dem-worker.js');
const terrain = terrainSource(absolute('/maps/terrain/{z}/{x}/{y}.webp')).acquire(maplibre);
const dem = terrain.dem;
const contourURL = dem.contourProtocolUrl({ thresholds: { 10: [200, 1000], 11: [100, 500], 13: [50, 250], 14: [20, 100] }, contourLayer: 'contours', elevationKey: 'ele', levelKey: 'level' });
window.addEventListener('securitypolicyviolation', event => report.external_requests.push(event.blockedURI));

function idle(map: maplibre.Map): Promise<void> {
    return new Promise((resolve, reject) => {
        const done = () => { clearTimeout(timer); map.off('idle', done); resolve(); };
        const timer = setTimeout(() => { map.off('idle', done); reject(new Error('Overlay did not become idle within60s')); }, 60_000);
        map.on('idle', done);
        if (map.loaded()) requestAnimationFrame(() => requestAnimationFrame(() => { if (map.loaded()) done(); }));
    });
}

const canonical = (value: any): any => Array.isArray(value) ? value.map(canonical)
    : value && typeof value === 'object' ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;

async function measure(map: maplibre.Map, overlays: RouteOverlays, action: () => Promise<unknown>) {
    const start = performance.now(), frames: number[] = [];
    let previous = start, animation = 0;
    const frame = (time: number) => { frames.push(time - previous); previous = time; animation = requestAnimationFrame(frame); };
    animation = requestAnimationFrame(frame);
    try { await action(); await idle(map); } finally { cancelAnimationFrame(animation); }
    const elapsed = performance.now() - start;
    frames.sort((a, b) => a - b);
    const result = { elapsed_ms: elapsed, frames: frames.length, frame_p95_ms: frames[Math.floor(frames.length * .95)] ?? 0,
        frame_max_ms: frames.at(-1) ?? 0, rendered: map.queryRenderedFeatures({ layers: ['network-cycling', 'network-hiking', 'hiking-markers'] }).length };
    if (!report.audit) return result;
    const data: any = await (map.getSource('route-overlays') as maplibre.GeoJSONSource).getData();
    const features = data.features.map((feature: any) => ({ ...feature, properties: overlays.selection(feature, [0, 0]) })).sort((a, b) => Number(a.id) - Number(b.id));
    const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(JSON.stringify(canonical(features))));
    return { ...result, features: features.length, sha256: [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('') };
}

async function run() {
    const config = await (await fetch('config.json')).json();
    report.versions = config.versions;
    report.audit = config.audit !== false;
    report.persistent_map = config.continuous === true;
    report.started_at = new Date().toISOString();
    report.renderer_sha256 = config.renderer_sha256;
    report.overlay_transport = 'Exact captured API replies; SQLite query time measured separately';
    report.archive = config.archives.maps.sha256;
    const container = document.getElementById('map')!;
    container.style.width = '390px'; container.style.height = '763px';
    report.viewport = { width: 390, height: 763, pixel_ratio: devicePixelRatio };
    let map: maplibre.Map | undefined, overlays: RouteOverlays | undefined;
    let activeName = '';
    const statuses: string[] = [];
    for (const [name, center, zoom] of [
        ['BW', [8.95, 48.65], 7], ['Regional', [8.95, 48.65], 8],
        ['Freiburg', [7.849, 47.997], 14], ['Feldberg', [8.005, 47.873], 13], ['Stuttgart', [9.182, 48.775], 12],
    ] as const) {
        send('progress', name);
        activeName = name;
        if (!map) {
            const style = mapStyle('light', dem.sharedDemProtocolUrl, contourURL);
            (style.sources.basemap as any).url = `pmtiles://${absolute('/maps/basemap.pmtiles')}`;
            for (const source of ['terrain', 'contours']) (style.sources[source] as any).bounds = config.archives.maps.bounds;
            style.glyphs = absolute('/maps/assets/fonts/{fontstack}/{range}.pbf'); style.sprite = absolute('/maps/assets/sprites/v4/light');
            map = new maplibre.Map({ container, style, center: [...center], zoom, maxBounds: config.archives.maps.bounds, attributionControl: false, maxPitch: 0, renderWorldCopies: false });
            map.on('error', event => report.errors.push({ name: activeName, message: event.error.message }));
            map.setMissingStyleImageResolver(id => { const icon = mapIcon(id); if (icon && !map!.hasImage(id)) map!.addImage(id, icon.image, { pixelRatio: icon.pixelRatio }); });
            await idle(map);
            overlays = new RouteOverlays(map, (message, retry) => { if (retry) statuses.push(message); });
            overlays.install('light');
        } else {
            overlays!.set({ network: 'none', access: false });
            map.jumpTo({ center: [...center], zoom });
            await idle(map);
        }
        statuses.length = 0;
        const currentMap = map, currentOverlays = overlays!;
        for (const network of ['cycling', 'hiking'] as const) {
            const load = await measure(currentMap, currentOverlays, async () => { currentOverlays.set({ network, access: false }); await currentOverlays.refresh(); });
            const pan = await measure(currentMap, currentOverlays, async () => { currentMap.panBy([200, 0], { duration: 0 }); await currentOverlays.refresh(); });
            const back = await measure(currentMap, currentOverlays, async () => { currentMap.jumpTo({ center: [...center], zoom }); await currentOverlays.refresh(); });
            report.samples.push({ name, network, load, pan, back, errors: [...statuses] });
        }
        if (!report.persistent_map) { overlays!.destroy(); map.remove(); map = undefined; overlays = undefined; }
    }
    if (!report.persistent_map) terrain.release();
    report.complete = true; report.finished_at = new Date().toISOString(); send('complete', report);
}
run().catch(error => { report.errors.push(String(error)); send('complete', report); });
