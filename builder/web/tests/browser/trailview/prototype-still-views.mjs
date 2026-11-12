// PROTOTYPE for #3361, never merged: routes a list of trips and writes a fly view from above and one
// at an angle that each hold the whole route, as jobs for prototype-still-cost.mjs.
//
//   node builder/web/tests/browser/trailview/prototype-still-views.mjs --routing URL --trips FILE --out DIR [--width 1920 --height 1080]
//
// --trips is a JSON list of {name, profile, points: [[lon, lat], ...]}. Writes DIR/<name>.json (the
// route line) and DIR/jobs.json; prints the length and the views of each route.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';

const { values: a } = parseArgs({ options: { routing: { type: 'string' }, trips: { type: 'string' }, out: { type: 'string' }, width: { type: 'string', default: '1920' }, height: { type: 'string', default: '1080' }, date: { type: 'string', default: '2026-08-15' }, time: { type: 'string', default: '15:00' } } });
const W = +a.width, H = +a.height, ASPECT = W / H, VFOV = 60, RAD = Math.PI / 180, R = 6_371_000;
/** The route keeps this share of the half frame on each side. */
const FILL = 0.9;
const tanV = Math.tan(VFOV / 2 * RAD), tanH = tanV * ASPECT;
mkdirSync(a.out, { recursive: true });

function decode(route) {
    const c = route.coordinates_udeg, e = route.elevation_dm, line = [], heights = [];
    let x = 0, y = 0, z = 0;
    for (let i = 0; i < c.length / 2; i++) {
        x += c[2 * i]; y += c[2 * i + 1]; if (e[i] !== null) z += e[i];
        line.push([x / 1e6, y / 1e6]); heights.push(z / 10);
    }
    return { line, heights };
}

/** Points in metres east, north and up of `o`, with the drop of the Earth's curvature. */
function local(line, heights, o) {
    const k = Math.cos(o[1] * RAD) * R * RAD;
    return line.map(([lon, lat], i) => {
        const e = (lon - o[0]) * k, n = (lat - o[1]) * R * RAD;
        return [e, n, heights[i] - (e * e + n * n) / (2 * R)];
    });
}

/** Whether every point is in the frame of an eye at `eye` with `heading` and `pitch`. */
function fits(points, eye, heading, pitch) {
    const h = heading * RAD, p = pitch * RAD;
    const f = [Math.sin(h) * Math.cos(p), Math.cos(h) * Math.cos(p), Math.sin(p)];
    const r = [Math.cos(h), -Math.sin(h), 0], u = [-Math.sin(h) * Math.sin(p), -Math.cos(h) * Math.sin(p), Math.cos(p)];
    return points.every(q => {
        const v = [q[0] - eye[0], q[1] - eye[1], q[2] - eye[2]], dot = w => v[0] * w[0] + v[1] * w[1] + v[2] * w[2];
        const z = dot(f);
        return z > 0 && Math.abs(dot(r) / z) < tanH * FILL && Math.abs(dot(u) / z) < tanV * FILL;
    });
}

/** The smallest `d` in (0, hi] for which `ok(d)`, by bisection. */
function smallest(ok, hi = 400_000) {
    let lo = 0;
    for (let i = 0; i < 40; i++) { const mid = (lo + hi) / 2; if (ok(mid)) hi = mid; else lo = mid; }
    return hi;
}

const toLonLat = (o, [e, n]) => [o[0] + e / (Math.cos(o[1] * RAD) * R * RAD), o[1] + n / (R * RAD)];
const query = (o, heading, pitch, alt) => `?lat=${o[1].toFixed(6)}&lon=${o[0].toFixed(6)}&heading=${heading.toFixed(1)}&pitch=${pitch.toFixed(1)}&alt=${alt.toFixed(0)}&date=${a.date}&time=${a.time}&weather=sunny`;

const jobs = [];
for (const trip of JSON.parse(readFileSync(a.trips, 'utf8'))) {
    const response = await fetch(`${a.routing}/v1/route`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ points: trip.points, profile: trip.profile }) });
    if (!response.ok) throw new Error(`${trip.name}: ${response.status} ${await response.text()}`);
    const route = (await response.json()).routes[0];
    const { line, heights } = decode(route);
    const file = join(a.out, `${trip.name}.json`);
    writeFileSync(file, JSON.stringify(line));
    const lons = line.map(p => p[0]), lats = line.map(p => p[1]);
    const centre = [(Math.min(...lons) + Math.max(...lons)) / 2, (Math.min(...lats) + Math.max(...lats)) / 2];
    const points = local(line, heights, centre), ground = heights.reduce((s, z) => s + z, 0) / heights.length;

    // From above: the eye over the centre of the bounds, north up.
    const top = smallest(d => fits(points, [0, 0, ground + d], 0, -89));
    // At an angle: the eye looks along the line from the start to the centre, at the centre.
    const s = points[0], heading = (Math.atan2(-s[0], -s[1]) / RAD + 360) % 360, pitch = -35;
    const dir = [Math.sin(heading * RAD), Math.cos(heading * RAD)], t = Math.tan(-pitch * RAD);
    const eyeAt = d => [-dir[0] * d, -dir[1] * d, ground + d * t];
    const back = smallest(d => fits(points, eyeAt(d), heading, pitch));
    const eye = eyeAt(back), behind = back > Math.hypot(s[0], s[1]);

    console.log(JSON.stringify({ name: trip.name, km: +(route.totals.distance_m / 1000).toFixed(1), points: line.length, top_alt: Math.round(ground + top), angle_alt: Math.round(eye[2]), angle_back_km: +(back / 1000).toFixed(1), eye_behind_start: behind }));
    jobs.push({ name: `${trip.name}-top`, view: query(centre, 0, -89, ground + top), width: W, height: H, route: file });
    jobs.push({ name: `${trip.name}-angle`, view: query(toLonLat(centre, eye), heading, pitch, eye[2]), width: W, height: H, route: file });
}
writeFileSync(join(a.out, 'jobs.json'), JSON.stringify(jobs, null, 1));
