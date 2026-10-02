import * as maplibre from 'maplibre-gl';
import { PMTiles, Protocol } from 'pmtiles';
import contour from 'maplibre-contour';
import { mapStyle } from '../../../../../builder/app/src/lib/planner/map-style';
import { mapIcon } from '../../../../../builder/app/src/lib/planner/map-icons';

const report: any = { samples: [], errors: [], missing_tiles: [], external_requests: [], animation_duration_ms: 500,
    readiness: 'MapLibre idle, including symbol fade completion', file_cache: 'Uncontrolled OS and WebKit caches',
    memory: 'Native process measurements exclude WebContent and GPU processes' };
const send = (kind: string, data: unknown) => {
    (window as any).webkit?.messageHandlers.mapBenchmark.postMessage({ kind, data });
    if (kind === 'progress') document.title = `Map benchmark: ${data}`;
    if (kind === 'complete') {
        document.title = 'Map benchmark complete';
        const output = document.createElement('pre');
        output.textContent = JSON.stringify(data, null, 2);
        document.body.replaceChildren(output);
        document.body.style.overflow = 'auto';
    }
};
const absolute = (value: string) => new URL(value, location.href).href;
const protocol = new Protocol();
const elevations = new Map<string, InstanceType<typeof contour.DemSource>>();
maplibre.setWorkerUrl(absolute('map-worker.js'));
maplibre.addProtocol('pmtiles', protocol.tile);
contour.workerUrl = absolute('dem-worker.js');
window.addEventListener('securitypolicyviolation', event => report.external_requests.push(event.blockedURI));

function settle(map: maplibre.Map, action?: () => void): Promise<any> {
    return new Promise((resolve, reject) => {
        const start = performance.now(), frames: number[] = [];
        let previous = start, animation = 0;
        const sample = (now: number) => { frames.push(now - previous); previous = now; animation = requestAnimationFrame(sample); };
        const cleanup = () => { cancelAnimationFrame(animation); clearTimeout(timer); map.off('idle', done); };
        const done = () => {
            cleanup();
            frames.sort((a, b) => a - b);
            resolve({ elapsed_ms: performance.now() - start, frames: frames.length, frame_p95_ms: frames[Math.floor(frames.length * .95)] ?? 0, frame_max_ms: frames.at(-1) ?? 0 });
        };
        const timer = setTimeout(() => { cleanup(); reject(new Error('Map did not become idle within 60 seconds')); }, 60_000);
        map.on('idle', done);
        animation = requestAnimationFrame(sample);
        action?.();
    });
}

async function run() {
    const config = await (await fetch('config.json')).json();
    report.archives = Object.fromEntries(Object.entries(config.archives).map(([name, value]: [string, any]) => [name, { bounds: value.bounds, manifest_sha256: value.sha256, basemap: value.files['basemap.pmtiles'], terrain: value.files['terrain.pmtiles'] }]));
    report.versions = config.versions;
    report.viewport = { width: innerWidth, height: innerHeight, pixel_ratio: devicePixelRatio };
    report.user_agent = navigator.userAgent;
    const cases = [
        { name: 'BW overview', directory: 'maps', center: [8.95, 48.65], zoom: 7 },
        { name: 'Freiburg', directory: 'maps', center: [7.849, 47.997], zoom: 14 },
        { name: 'Feldberg', directory: 'maps', center: [8.005, 47.873], zoom: 13 },
        { name: 'Stuttgart', directory: 'maps', center: [9.182, 48.775], zoom: 12 },
        { name: 'Cutout boundary', directory: 'map-cutout', center: [7.77, 47.965], zoom: 14 },
    ];
    for (const entry of cases) {
        send('progress', entry.name);
        const errorStart = report.errors.length;
        const url = absolute(`/${entry.directory}/basemap.pmtiles`);
        const archive = new PMTiles(url);
        const read = archive.getZxy.bind(archive);
        archive.getZxy = async (...args) => {
            const tile = await read(...args);
            if (!tile) report.missing_tiles.push({ case: entry.name, source: 'basemap', zxy: args.slice(0, 3) });
            return tile;
        };
        protocol.add(archive);
        let dem = elevations.get(entry.directory);
        if (!dem) {
            dem = new contour.DemSource({ id: `dem-${entry.directory}`, url: absolute(`/${entry.directory}/terrain/{z}/{x}/{y}.webp`).replaceAll('%7B', '{').replaceAll('%7D', '}'), maxzoom: 12, worker: true, cacheSize: 64, encoding: 'terrarium' });
            dem.setupMaplibre(maplibre);
            elevations.set(entry.directory, dem);
        }
        const contours = dem.contourProtocolUrl({ thresholds: { 10: [200, 1000], 11: [100, 500], 13: [50, 250], 14: [20, 100] }, contourLayer: 'contours', elevationKey: 'ele', levelKey: 'level' });
        const style = mapStyle('light', dem.sharedDemProtocolUrl, contours);
        style.glyphs = absolute('/maps/assets/fonts/{fontstack}/{range}.pbf').replaceAll('%7B', '{').replaceAll('%7D', '}');
        style.sprite = absolute('/maps/assets/sprites/v4/light');
        (style.sources.basemap as any).url = `pmtiles://${url}`;
        for (const name of ['terrain', 'contours']) (style.sources[name] as any).bounds = config.archives[entry.directory].bounds;
        const map = new maplibre.Map({ container: 'map', style, center: entry.center as [number, number], zoom: entry.zoom,
            maxBounds: config.archives[entry.directory].bounds, attributionControl: false, maxPitch: 0, renderWorldCopies: false });
        map.on('error', event => report.errors.push({ case: entry.name, message: event.error.message }));
        map.setMissingStyleImageResolver(id => { const icon = mapIcon(id); if (icon && !map.hasImage(id)) map.addImage(id, icon.image, { pixelRatio: icon.pixelRatio }); });
        try {
            const load = await settle(map);
            const pan = await settle(map, () => map.panBy([80, 40], { duration: 500 }));
            const zoom = await settle(map, () => map.zoomTo(entry.zoom + 1, { duration: 500 }));
            const warm = await settle(map, () => map.jumpTo({ center: entry.center as [number, number], zoom: entry.zoom }));
            report.samples.push({ name: entry.name, directory: entry.directory, center: entry.center, start_zoom: entry.zoom,
                load, pan, zoom, warm, errors: report.errors.length - errorStart,
                rendered_features: map.queryRenderedFeatures().length,
                contour_features: map.queryRenderedFeatures({ layers: ['contour-lines'] }).length,
                hillshade_layer: map.getLayer('relief')?.type,
                terrain_loaded: map.isSourceLoaded('terrain'), contours_loaded: map.isSourceLoaded('contours') });
            if (entry.name === 'Cutout boundary') await new Promise(resolve => setTimeout(resolve, 100));
        } catch (error) { report.errors.push({ case: entry.name, message: String(error) }); }
        map.remove();
    }
    report.complete = true;
    report.dem_cache = { entries_per_region: 64, retained_regions: elevations.size };
    (window as any).plannerMapReport = report;
    send('complete', report);
}
run().catch(error => { report.errors.push(String(error)); send('complete', report); });
