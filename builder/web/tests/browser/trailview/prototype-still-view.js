// PROTOTYPE for #3361, never merged: chooses the cover view of a route in the browser.
// Rule B: for each heading, the lowest tilt that shows MIN of the route's length; of those, the
// heading that draws the route longest on screen. Then the route is centred in the frame. The eye
// never looks within SUN_DEG of the sun: into the sun, water turns white and the haze grows.

const RAD = Math.PI / 180, R = 6_371_000, VFOV = 60, FILL = 0.9, TILE = 512;
const HEADINGS = Array.from({ length: 12 }, (_, i) => 30 * i);
const FLAT = -20, STEEP = -89;
/** Points along the route; 200 resolve the share to 0.5 %. */
const POINTS = 200;
/** The ground hides a point when it rises this far above the line of sight: below zero, so trees of about 20 m hide too. */
const CLEAR_M = -20;
/** The height grid holds about this many cells across the area, from at most four tiles. */
const CELLS = 600;
const SUN_DEG = 60;

/** Tile pixel coordinates at zoom z. */
const mercX = (lon, z) => (lon + 180) / 360 * TILE * 2 ** z;
const mercY = (lat, z) => (1 - Math.log(Math.tan(Math.PI / 4 + lat * RAD / 2)) / Math.PI) / 2 * TILE * 2 ** z;
const latOf = (y, z) => Math.atan(Math.sinh(Math.PI * (1 - 2 * y / (TILE * 2 ** z)))) / RAD;

/** The heights of a box as one grid of tile pixels, in metres, with the Earth's curvature about `centre` taken off. */
async function heightGrid(box, centre, template) {
    const span = Math.max(box[2] - box[0], (box[3] - box[1]) / Math.cos(centre[1] * RAD));
    const z = Math.max(0, Math.min(12, Math.floor(Math.log2(CELLS * 360 / (TILE * span)))));
    const x0 = Math.floor(mercX(box[0], z)), x1 = Math.ceil(mercX(box[2], z)), y0 = Math.floor(mercY(box[3], z)), y1 = Math.ceil(mercY(box[1], z));
    const width = x1 - x0, height = y1 - y0, grid = new Float32Array(width * height);
    const t0 = performance.now();
    const tiles = [];
    for (let tx = Math.floor(x0 / TILE); tx <= Math.floor((x1 - 1) / TILE); tx++)
        for (let ty = Math.floor(y0 / TILE); ty <= Math.floor((y1 - 1) / TILE); ty++) tiles.push([tx, ty]);
    const images = await Promise.all(tiles.map(async ([tx, ty]) => {
        const response = await fetch(template.replace('{z}', z).replace('{x}', tx).replace('{y}', ty));
        return response.status === 200 ? createImageBitmap(await response.blob(), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' }) : undefined;
    }));
    const fetched = performance.now();
    const canvas = new OffscreenCanvas(TILE, TILE), context = canvas.getContext('2d', { willReadFrequently: true });
    const k = Math.cos(centre[1] * RAD) * R * RAD, m = R * RAD;
    const east = Float32Array.from({ length: width }, (_, i) => ((x0 + i + 0.5) / (TILE * 2 ** z) * 360 - 180 - centre[0]) * k);
    const north = Float32Array.from({ length: height }, (_, j) => (latOf(y0 + j + 0.5, z) - centre[1]) * m);
    tiles.forEach(([tx, ty], i) => {
        if (!images[i]) return;
        context.drawImage(images[i], 0, 0);
        const rgba = context.getImageData(0, 0, TILE, TILE).data;
        for (let py = 0; py < TILE; py++) {
            const gy = ty * TILE + py - y0;
            if (gy < 0 || gy >= height) continue;
            for (let px = 0; px < TILE; px++) {
                const gx = tx * TILE + px - x0;
                if (gx < 0 || gx >= width) continue;
                const o = 4 * (py * TILE + px), e = east[gx], n = north[gy];
                grid[gy * width + gx] = rgba[o] * 256 + rgba[o + 1] + rgba[o + 2] / 256 - 32768 - (e * e + n * n) / (2 * R);
            }
        }
    });
    return { z, x0, y0, width, height, grid, k, m, tiles: tiles.length, fetchMs: fetched - t0, decodeMs: performance.now() - fetched };
}

/** `count` points evenly spaced along the line. */
function resample(line, count) {
    const d = [0];
    for (let i = 1; i < line.length; i++) d.push(d[i - 1] + Math.hypot((line[i][0] - line[i - 1][0]) * Math.cos(line[i][1] * RAD), line[i][1] - line[i - 1][1]));
    const out = [];
    for (let s = 0, i = 0; s < count; s++) {
        const t = d.at(-1) * s / (count - 1);
        while (i < line.length - 2 && d[i + 1] < t) i++;
        const f = d[i + 1] > d[i] ? (t - d[i]) / (d[i + 1] - d[i]) : 0;
        out.push([line[i][0] + f * (line[i + 1][0] - line[i][0]), line[i][1] + f * (line[i + 1][1] - line[i][1])]);
    }
    return out;
}

/** The sun's azimuth in degrees from north at `time` (a Date), seen from `lat`, `lon`; NOAA's short formulas. */
export function sunAzimuth(time, lat, lon) {
    const day = (time - Date.UTC(time.getUTCFullYear(), 0, 1)) / 86_400_000, g = 2 * Math.PI / 365 * day;
    const decl = 0.006918 - 0.399912 * Math.cos(g) + 0.070257 * Math.sin(g) - 0.006758 * Math.cos(2 * g) + 0.000907 * Math.sin(2 * g);
    const eqt = 229.18 * (0.000075 + 0.001868 * Math.cos(g) - 0.032077 * Math.sin(g) - 0.014615 * Math.cos(2 * g) - 0.040849 * Math.sin(2 * g));
    const solar = time.getUTCHours() * 60 + time.getUTCMinutes() + eqt + 4 * lon, ha = (solar / 4 - 180) * RAD, phi = lat * RAD;
    const zen = Math.acos(Math.sin(phi) * Math.sin(decl) + Math.cos(phi) * Math.cos(decl) * Math.cos(ha));
    const az = Math.acos((Math.sin(decl) - Math.sin(phi) * Math.cos(zen)) / (Math.cos(phi) * Math.sin(zen))) / RAD;
    return ha > 0 ? 360 - az : az;
}

const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
function axes(heading, pitch) {
    const h = heading * RAD, p = pitch * RAD;
    return { f: [Math.sin(h) * Math.cos(p), Math.cos(h) * Math.cos(p), Math.sin(p)], r: [Math.cos(h), -Math.sin(h), 0], u: [-Math.sin(h) * Math.sin(p), -Math.cos(h) * Math.sin(p), Math.cos(p)] };
}

/**
 * The cover view of `line` ([[lon, lat], ...]) for an image of `width` × `height`, from the terrain
 * tiles of `template` (`.../{z}/{x}/{y}.webp`, 512 px Terrarium), lit by the sun at `time`. Resolves to
 * the view and its timings.
 */
export async function chooseView(line, template, { width = 1920, height = 1080, min = 0.85, time = new Date('2026-08-15T13:00:00Z') } = {}) {
    const t0 = performance.now();
    const tanV = Math.tan(VFOV / 2 * RAD), tanH = tanV * width / height;
    const lons = line.map(p => p[0]), lats = line.map(p => p[1]);
    const box = [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)];
    const centre = [(box[0] + box[2]) / 2, (box[1] + box[3]) / 2];
    // The eye of a low tilt stands outside the route's box: the grid reaches 0.8 of the box beyond it.
    const pad = 0.8 * Math.max(box[2] - box[0], (box[3] - box[1]) / Math.cos(centre[1] * RAD), 0.05);
    const g = await heightGrid([box[0] - pad, box[1] - pad * Math.cos(centre[1] * RAD), box[2] + pad, box[3] + pad * Math.cos(centre[1] * RAD)], centre, template);
    const t1 = performance.now();

    // Local metres about the centre, and grid cells, of a point given in metres.
    const cellX = e => mercX(centre[0] + e / g.k, g.z) - g.x0, cellY = n => mercY(centre[1] + n / g.m, g.z) - g.y0;
    const groundAt = (x, y) => {
        const i = Math.floor(x), j = Math.floor(y);
        return i < 0 || j < 0 || i >= g.width || j >= g.height ? -Infinity : g.grid[j * g.width + i];
    };
    const points = resample(line, POINTS).map(([lon, lat]) => {
        const e = (lon - centre[0]) * g.k, n = (lat - centre[1]) * g.m, x = cellX(e), y = cellY(n);
        return { p: [e, n, groundAt(x, y) + 1], x, y };
    });
    const base = points.reduce((s, q) => s + q.p[2], 0) / points.length;
    const allowed = Math.floor((1 - min) * points.length);
    let views = 0;

    /** The share of points that no ground hides from `eye`; below `min` it stops early and returns 0. */
    function seen(eye) {
        views++;
        const ex = cellX(eye[0]), ey = cellY(eye[1]);
        let hidden = 0;
        for (const q of points) {
            const dx = q.x - ex, dy = q.y - ey, steps = Math.ceil(Math.max(Math.abs(dx), Math.abs(dy)));
            // The last two cells hold the point's own ground.
            for (let s = 1; s < steps - 2; s++) {
                const t = s / steps;
                if (groundAt(ex + t * dx, ey + t * dy) > eye[2] + t * (q.p[2] - eye[2]) + CLEAR_M) { hidden++; break; }
            }
            if (hidden > allowed) return 0;
        }
        return 1 - hidden / points.length;
    }
    /** Screen coordinates of every point, from -1 to 1 across the frame. */
    function project(eye, { f, r, u }) {
        return points.map(({ p }) => {
            const v = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]], z = dot(v, f);
            return z > 0 ? [dot(v, r) / z / tanH, dot(v, u) / z / tanV] : [Infinity, Infinity];
        });
    }
    /** The nearest eye on the line through `target` that holds every point in the frame. */
    function fit(target, frame) {
        let lo = 0, hi = 2_000_000;
        const eyeAt = d => [target[0] - frame.f[0] * d, target[1] - frame.f[1] * d, target[2] - frame.f[2] * d];
        for (let i = 0; i < 24; i++) {
            const mid = (lo + hi) / 2;
            if (project(eyeAt(mid), frame).every(([x, y]) => Math.abs(x) < FILL && Math.abs(y) < FILL)) hi = mid; else lo = mid;
        }
        return { eye: eyeAt(hi), d: hi };
    }
    /** Moves the target until the route's extent on screen is centred, then fits again. */
    function centred(frame) {
        let target = [0, 0, base], view = fit(target, frame);
        for (let i = 0; i < 3; i++) {
            const s = project(view.eye, frame), xs = s.map(q => q[0]), ys = s.map(q => q[1]);
            const cx = (Math.min(...xs) + Math.max(...xs)) / 2, cy = (Math.min(...ys) + Math.max(...ys)) / 2;
            target = target.map((t, j) => t + view.d * (cx * tanH * frame.r[j] + cy * tanV * frame.u[j]));
            view = fit(target, frame);
        }
        return view;
    }
    const length = (eye, frame) => {
        const s = project(eye, frame);
        let sum = 0;
        for (let i = 1; i < s.length; i++) sum += Math.hypot((s[i][0] - s[i - 1][0]) * width, (s[i][1] - s[i - 1][1]) * height) / 2;
        return sum;
    };

    const sun = sunAzimuth(time, centre[1], centre[0]);
    let best;
    for (const heading of HEADINGS.filter(h => Math.abs(((h - sun) % 360 + 540) % 360 - 180) >= SUN_DEG)) {
        const at = pitch => { const frame = axes(heading, pitch), view = centred(frame); return { heading, pitch, frame, ...view, share: seen(view.eye) }; };
        // The share grows as the tilt steepens: halve the range of tilts down to 2°.
        let pass = at(FLAT);
        if (!pass.share) {
            let flat = FLAT, steep = STEEP;
            pass = at(STEEP);
            if (!pass.share) continue;
            while (flat - steep > 2) {
                const mid = Math.round((flat + steep) / 2), view = at(mid);
                if (view.share) { steep = mid; pass = view; } else flat = mid;
            }
        }
        pass.length = length(pass.eye, pass.frame);
        if (!best || pass.length > best.length) best = pass;
    }
    const t2 = performance.now();
    const lonlat = [centre[0] + best.eye[0] / g.k, centre[1] + best.eye[1] / g.m];
    return {
        view: { lat: lonlat[1], lon: lonlat[0], heading: best.heading, pitch: best.pitch, alt: best.eye[2] },
        share: best.share, sun: Math.round(sun), zoom: g.z, tiles: g.tiles, cells: [g.width, g.height], views,
        ms: { fetch: Math.round(g.fetchMs), decode: Math.round(g.decodeMs), search: Math.round(t2 - t1), total: Math.round(t2 - t0) },
    };
}
