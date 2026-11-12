// PROTOTYPE for #3361, never merged: finds the most angled fly view that holds a whole route and
// still shows a share of its length that the terrain does not hide.
//
//   node builder/web/tests/browser/trailview/prototype-still-angle.mjs --terrain URL --routes A.json,B.json --out DIR [--min 0.85]
//
// --terrain is the release's terrain template, `.../terrain/{z}/{x}/{y}.webp` (512 px Terrarium).
// Each route is a file of [[lon, lat], ...]. Writes DIR/angle-jobs.json for prototype-still-cost.mjs
// and prints, for each route, the chosen view and the visible share of every tilt at its heading.
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { parseArgs } from 'node:util';

const { values: a } = parseArgs({ options: { terrain: { type: 'string' }, routes: { type: 'string' }, out: { type: 'string' }, min: { type: 'string', default: '0.85' }, width: { type: 'string', default: '1920' }, height: { type: 'string', default: '1080' } } });
const W = +a.width, H = +a.height, VFOV = 60, RAD = Math.PI / 180, R = 6_371_000, FILL = 0.9, MIN = +a.min;
const tanV = Math.tan(VFOV / 2 * RAD), tanH = tanV * W / H;
/** Samples along the route; the search uses fewer. */
const SAMPLES = 2000, SEARCH = 400;
/** A point counts as hidden when the ground rises this far above the line of sight before it. */
const CLEAR_M = 5;
const PITCHES = Array.from({ length: 14 }, (_, i) => -20 - 5 * i).concat(-89);
const HEADINGS = Array.from({ length: 12 }, (_, i) => 30 * i);
const cache = join(a.out, 'dem');
mkdirSync(cache, { recursive: true });

/** Heights of one terrain tile, row-major 512 × 512, or undefined for an absent tile. */
async function tile(z, x, y) {
    const file = join(cache, `${z}-${x}-${y}.webp`);
    if (!existsSync(file)) {
        const response = await fetch(a.terrain.replace('{z}', z).replace('{x}', x).replace('{y}', y));
        writeFileSync(file, response.status === 200 ? Buffer.from(await response.arrayBuffer()) : Buffer.alloc(0));
    }
    if (readFileSync(file).length === 0) return undefined;
    const rgb = execFileSync('magick', [file, '-depth', '8', 'rgb:-'], { maxBuffer: 2 ** 24 });
    const h = new Float32Array(512 * 512);
    for (let i = 0; i < h.length; i++) h[i] = rgb[3 * i] * 256 + rgb[3 * i + 1] + rgb[3 * i + 2] / 256 - 32768;
    return h;
}

/** A height function over the bounds at zoom z: bilinear between pixel centres, 0 where absent. */
async function dem(bounds, z) {
    const n = 2 ** z, tx = lon => (lon + 180) / 360 * n, ty = lat => (1 - Math.log(Math.tan(Math.PI / 4 + lat * RAD / 2)) / Math.PI) / 2 * n;
    const tiles = new Map();
    for (let x = Math.floor(tx(bounds[0])); x <= Math.floor(tx(bounds[2])); x++)
        for (let y = Math.floor(ty(bounds[3])); y <= Math.floor(ty(bounds[1])); y++) tiles.set(`${x}/${y}`, await tile(z, x, y));
    const px = (X, Y) => {
        const x = Math.floor(X / 512), y = Math.floor(Y / 512), t = tiles.get(`${x}/${y}`);
        return t ? t[(Y - 512 * y) * 512 + (X - 512 * x)] : 0;
    };
    return (lon, lat) => {
        const X = tx(lon) * 512 - 0.5, Y = ty(lat) * 512 - 0.5, x0 = Math.floor(X), y0 = Math.floor(Y), fx = X - x0, fy = Y - y0;
        return (px(x0, y0) * (1 - fx) + px(x0 + 1, y0) * fx) * (1 - fy) + (px(x0, y0 + 1) * (1 - fx) + px(x0 + 1, y0 + 1) * fx) * fy;
    };
}

/** `count` points evenly spaced along the line. */
function resample(line, count) {
    const d = [0];
    for (let i = 1; i < line.length; i++) d.push(d[i - 1] + Math.hypot((line[i][0] - line[i - 1][0]) * Math.cos(line[i][1] * RAD), line[i][1] - line[i - 1][1]));
    const out = [];
    for (let k = 0, i = 0; k < count; k++) {
        const t = d.at(-1) * k / (count - 1);
        while (i < line.length - 2 && d[i + 1] < t) i++;
        const f = d[i + 1] > d[i] ? (t - d[i]) / (d[i + 1] - d[i]) : 0;
        out.push([line[i][0] + f * (line[i + 1][0] - line[i][0]), line[i][1] + f * (line[i + 1][1] - line[i][1])]);
    }
    return out;
}

function frame(heading, pitch) {
    const h = heading * RAD, p = pitch * RAD;
    return { f: [Math.sin(h) * Math.cos(p), Math.cos(h) * Math.cos(p), Math.sin(p)], r: [Math.cos(h), -Math.sin(h), 0], u: [-Math.sin(h) * Math.sin(p), -Math.cos(h) * Math.sin(p), Math.cos(p)] };
}
const dot = (v, w) => v[0] * w[0] + v[1] * w[1] + v[2] * w[2];
function fits(points, eye, { f, r, u }) {
    return points.every(q => {
        const v = [q[0] - eye[0], q[1] - eye[1], q[2] - eye[2]], z = dot(v, f);
        return z > 0 && Math.abs(dot(v, r) / z) < tanH * FILL && Math.abs(dot(v, u) / z) < tanV * FILL;
    });
}
function smallest(ok, hi = 400_000) {
    let lo = 0;
    for (let i = 0; i < 40; i++) { const mid = (lo + hi) / 2; if (ok(mid)) hi = mid; else lo = mid; }
    return hi;
}

const routes = a.routes.split(',');
const jobs = [];
for (const path of routes) {
    const name = basename(path, '.json'), line = JSON.parse(readFileSync(path, 'utf8'));
    const lons = line.map(p => p[0]), lats = line.map(p => p[1]);
    const box = [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)];
    const centre = [(box[0] + box[2]) / 2, (box[1] + box[3]) / 2], k = Math.cos(centre[1] * RAD) * R * RAD;
    const span = Math.max((box[2] - box[0]) * k, (box[3] - box[1]) * R * RAD);
    // About 100 m pixels for the longest route, about 25 m for a short one; the margin holds the eye's line.
    const z = span > 60_000 ? 9 : span > 15_000 ? 10 : 11, m = Math.max(0.15, span / 111_000);
    const height = await dem([box[0] - m * 2, box[1] - m, box[2] + m * 2, box[3] + m], z);
    const toLocal = ([lon, lat], up) => { const e = (lon - centre[0]) * k, n = (lat - centre[1]) * R * RAD; return [e, n, up - (e * e + n * n) / (2 * R)]; };
    const ground = (e, n) => height(centre[0] + e / k, centre[1] + n / (R * RAD)) - (e * e + n * n) / (2 * R);
    const all = resample(line, SAMPLES).map(p => toLocal(p, height(...p) + 1));
    const few = all.filter((_, i) => i % (SAMPLES / SEARCH) === 0);
    const base = all.reduce((s, p) => s + p[2], 0) / all.length;

    /** The share of points that no ground hides from `eye`. */
    const visible = (points, eye) => points.filter(q => {
        const d = Math.hypot(q[0] - eye[0], q[1] - eye[1]), steps = Math.max(20, Math.ceil(d / 60));
        for (let s = 1; s < steps - 2; s++) {
            const t = s / steps, e = eye[0] + t * (q[0] - eye[0]), n = eye[1] + t * (q[1] - eye[1]), up = eye[2] + t * (q[2] - eye[2]);
            if (ground(e, n) > up + CLEAR_M) return false;
        }
        return true;
    }).length / points.length;
    /** The eye on the line through the centre at `heading` and `pitch`, as near as the route permits. */
    const view = (heading, pitch) => {
        const fr = frame(heading, pitch), eyeAt = d => [-fr.f[0] * d, -fr.f[1] * d, base - fr.f[2] * d];
        return eyeAt(smallest(d => fits(few, eyeAt(d), fr)));
    };

    /** The length of the route line on screen, in pixels. */
    const pixels = (points, eye, { f, r, u }) => {
        let sum = 0, last;
        for (const q of points) {
            const v = [q[0] - eye[0], q[1] - eye[1], q[2] - eye[2]], z = dot(v, f), s = [dot(v, r) / z / tanH * W / 2, dot(v, u) / z / tanV * H / 2];
            if (last) sum += Math.hypot(s[0] - last[0], s[1] - last[1]);
            last = s;
        }
        return sum;
    };
    const table = [];
    for (const heading of HEADINGS) for (const pitch of PITCHES) {
        const eye = view(heading, pitch);
        table.push({ heading, pitch, eye, seen: visible(few, eye), px: pixels(few, eye, frame(heading, pitch)) });
    }
    // For each heading, the shallowest tilt that shows MIN of the route.
    const best = HEADINGS.map(h => table.filter(t => t.heading === h && t.seen >= MIN).sort((p, q) => q.pitch - p.pitch)[0]).filter(Boolean);
    const top = table.find(t => t.pitch === -89 && t.heading === 0).px;
    // A: the shallowest of all, the longest line among equals. B: the longest line, at its own shallowest tilt.
    const A = [...best].sort((p, q) => q.pitch - p.pitch || q.px - p.px)[0], B = [...best].sort((p, q) => q.px - p.px)[0];
    for (const [tag, t] of [['a', A], ['b', B]]) {
        const lonlat = [centre[0] + t.eye[0] / k, centre[1] + t.eye[1] / (R * RAD)];
        console.log(JSON.stringify({ name, tag, heading: t.heading, pitch: t.pitch, alt: Math.round(t.eye[2]), seen: +visible(all, t.eye).toFixed(3), line_vs_top: +(t.px / top).toFixed(2) }));
        jobs.push({ name: `${name}-${tag}-h${t.heading}-p${-t.pitch}`, view: `?lat=${lonlat[1].toFixed(6)}&lon=${lonlat[0].toFixed(6)}&heading=${t.heading.toFixed(1)}&pitch=${t.pitch.toFixed(1)}&alt=${t.eye[2].toFixed(0)}&date=2026-08-15&time=15:00&weather=sunny&scale=0.75`, width: W, height: H, route: path });
    }
}
writeFileSync(join(a.out, 'angle-jobs.json'), JSON.stringify(jobs, null, 1));
