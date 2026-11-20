// Data, search, filter controls and map of the Explore mock; index.html draws the results table.
window.OBC = (() => {
const ICON = {
    check: 'm5 12 4 4L19 6', close: 'm6 6 12 12M6 18 18 6', lock: 'M6 11h12v9H6zM8 11V8a4 4 0 0 1 8 0v3', plus: 'M5 12h14m-7-7v14', minus: 'M5 12h14',
    down: 'M12 5v14m-6-6 6 6 6-6', pin: 'M12 21s-7-6.2-7-11a7 7 0 0 1 14 0c0 4.8-7 11-7 11Zm0-13.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5Z',
    locate: 'M12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10Zm0-5v3m0 14v3M2 12h3m14 0h3',
    binoculars: 'M9 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0Zm12 0a3 3 0 1 1-6 0 3 3 0 0 1 6 0ZM3 16V8a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v8m6 0V8a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v8M9 11h6M5 6V4h2v2m10 0V4h2v2',
    mountain: 'm2 20 7-13 4 7 3-5 6 11H2Zm4-7 3 1 2-3', ridge: 'M2 18l5-7 4 3 5-8 6 12',
    lake: 'M3 10c2-2 4-2 6 0s4 2 6 0 4-2 6 0M3 15c2-2 4-2 6 0s4 2 6 0 4-2 6 0', hut: 'M4 11l8-6 8 6v8H4zM10 19v-5h4v5',
    waterfall: 'M7 3v8M12 3v11M17 3v8M4 19c2-2 4-2 6 0s4 2 6 0 3-1 4 0', eye: 'M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12Zm10-3a3 3 0 1 0 0 6 3 3 0 0 0 0-6',
    flag: 'M5 22V3m0 1c5-4 9 4 15 0v10c-6 4-10-4-15 0', slope: 'M3 17c6-1 11-3 18-5M3 21h18',
    snow: 'M12 2v20M3.3 7l17.4 10M3.3 17 20.7 7M9.5 3.5 12 6l2.5-2.5M9.5 20.5 12 18l2.5 2.5M18.1 5.6 17.2 9l3.4.9M20.6 14.1 17.2 15l.9 3.4M3.4 14.1 6.8 15l-.9 3.4M5.9 5.6 6.8 9l-3.4.9',
    tree: 'M12 3l5 8h-3l4 6H6l4-6H7zM12 17v4', asph: 'M3 19h18M3 14h18M6 9h12', quiet: 'M5 20 9 4m10 16L15 4M12 6v2m0 3v2m0 3v2', up: 'm4 18 5-6 4 3 7-9M15 6h5v5',
    car: 'M3 15.5h18v3.5H3zM5 15.5l2-5.5h10l2 5.5M6 19v2m12-2v2M7 17.3h1m8 0h1', walk: 'M13.5 5a1.8 1.8 0 1 0 0-3.6 1.8 1.8 0 0 0 0 3.6ZM9 22l2.5-7 2.5 2.5V22M8 12l2.5-4.5h3.5l1.5 3.5 3 1.5M10.5 7.5 9.5 15',
    lift: 'M2 3l20 5M12 5.5V10M7 10h10l-1 10H8zM7.5 14h9', train: 'M7 3h10a2 2 0 0 1 2 2v9a3 3 0 0 1-3 3H8a3 3 0 0 1-3-3V5a2 2 0 0 1 2-2ZM5 10h14M8 21l2-4m6 4-2-4M9 13.5h.01M15 13.5h.01',
    info: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Zm0 8v5m0-8.5v.01', calendar: 'M4 6h16v14H4zM4 10h16M8 3v4m8-4v4',
};
const icon = (name, size = 18) => `<svg class="i" width="${size}" height="${size}" viewBox="0 0 24 24" aria-hidden="true"><path d="${ICON[name]}"/></svg>`;
const fmt = n => n.toLocaleString('en');
const PX_KM = 11, ROAD = 1.35, NS = 'http://www.w3.org/2000/svg', MAP_W = 1164, MAP_H = 892;
const DAYS = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31], MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
const day = (d, m) => DAYS.slice(0, m - 1).reduce((a, b) => a + b, 0) + d;
const dateText = (doy, short) => { let m = 0; while (doy > DAYS[m]) doy -= DAYS[m++]; return doy + ' ' + (short ? MONTHS[m].slice(0, 3) : MONTHS[m]); };
const TODAY = day(19, 11);
const DATES = [TODAY, day(1, 5), day(15, 5), day(1, 6), day(21, 6), day(5, 7), day(20, 7), day(10, 8), day(1, 9), day(20, 9), day(10, 10), day(1, 11)];
const WALK = [0.2, 0.3, 0.5, 0.75, 1, 1.5, 2, 3, 4, 5], ROAD_STEP = 5, ROAD_MAX = 100;
const dist = k => k < 0.95 ? Math.max(100, Math.round(k * 10) * 100) + ' m' : (Math.round(k * 10) / 10) + ' km';

// Towns give a picked point a name ("near Engelberg"). "My location" puts the point in Engelberg in this mock.
const TOWNS = [['Lucerne', 513, 373], ['Engelberg', 595, 680], ['Stans', 571, 507], ['Sarnen', 457, 593], ['Arth', 721, 357], ['Schwyz', 845, 417], ['Altdorf', 833, 610], ['Zug', 715, 212], ['Kriens', 495, 398], ['Sursee', 323, 205], ['Küssnacht', 640, 330], ['Ebikon', 545, 333]];
const GPS = { x: 595, y: 682 };
const townNear = p => TOWNS.reduce((a, t) => Math.hypot(t[1] - p.x, t[2] - p.y) < Math.hypot(a[1] - p.x, a[2] - p.y) ? t : a)[0];
// The size of a car park is an estimate from its mapped area, so the page shows only small or large.
const isLarge = park => park.spaces >= 50;

// Sample hikes. `f` lists what the hike has; `snow` is the usual snow-free window as days of the year; `p` is the car park at the start.
// A there-and-back hike with `to` ends at a point of another hike, so that two hikes share a summit.
const HIKES = [
    { id: 1, n: 'Ridge and summit loop', made: 0, km: 13.4, up: 920, t: 3, alt: 1898, p: 60, pn: 'Sattel', asph: 8, f: 'loop peak ridge views hut', snow: [day(15, 5), day(6, 11)], x: 745, y: 370, dir: 0, c: '#0d6e74' },
    { id: 2, n: 'Lakeside loop', made: 0, km: 8.2, up: 140, t: 1, alt: 540, p: 120, pn: 'Seeweg', asph: 34, f: 'loop lake forest safe', snow: [1, 365], x: 345, y: 232, dir: 20, c: '#2f62c9' },
    { id: 3, n: 'Hut loop over the alp', made: 1, km: 11.6, up: 680, t: 2, alt: 1620, p: 35, pn: 'Alpstrasse', asph: 6, f: 'loop hut views quiet safe', snow: [day(20, 5), day(1, 11)], x: 590, y: 545, dir: 90, c: '#c2278f' },
    { id: 4, n: 'Summit and hut loop', made: 1, km: 14.8, up: 1050, t: 3, alt: 2120, p: 25, pn: 'Talboden', asph: 4, f: 'loop peak hut ridge views quiet', snow: [day(14, 6), day(17, 10)], x: 455, y: 470, dir: 150, c: '#c8501e' },
    { id: 5, n: 'Gorge and waterfall loop', made: 0, km: 7.1, up: 380, t: 2, alt: 980, p: 40, pn: 'Schlucht', asph: 15, f: 'loop waterfall forest', snow: [day(10, 4), day(26, 11)], x: 330, y: 480, dir: 180, c: '#6a3fb0' },
    { id: 6, n: 'Summit, there and back', made: 0, km: 9.6, up: 870, t: 3, alt: 1898, p: 80, pn: 'Passhöhe', asph: 5, f: 'peak views', snow: [day(30, 5), day(27, 10)], x: 850, y: 380, to: [1, 5], c: '#8c1d2b' },
    { id: 7, n: 'Forest loop', made: 1, km: 10.2, up: 310, t: 1, alt: 790, p: 15, pn: 'Waldrand', asph: 12, f: 'loop forest quiet safe', snow: [day(16, 3), day(11, 12)], x: 450, y: 350, dir: 190, c: '#2f7d4f' },
    { id: 8, n: 'High ridge loop', made: 0, km: 16.5, up: 1240, t: 4, alt: 2404, p: 30, pn: 'Hinterberg', asph: 2, f: 'loop peak ridge views quiet', snow: [day(29, 6), day(7, 10)], x: 650, y: 650, dir: 80, c: '#0d6e74' },
    { id: 9, n: 'Lake and hut loop', made: 1, km: 12.3, up: 560, t: 2, alt: 1340, p: 50, pn: 'Seeboden', asph: 9, f: 'loop lake hut forest safe', snow: [day(5, 5), day(11, 11)], x: 405, y: 640, dir: 170, c: '#2f62c9' },
    { id: 10, n: 'Alp and waterfall loop', made: 0, km: 6.4, up: 420, t: 2, alt: 1510, p: 20, pn: 'Alpweg', asph: 7, f: 'loop waterfall hut views safe', snow: [day(15, 5), day(1, 11)], x: 520, y: 700, dir: 90, c: '#c2278f' },
    { id: 11, n: 'Lake shore, there and back', made: 0, km: 5.8, up: 90, t: 1, alt: 470, p: 0, asph: 41, f: 'lake safe', snow: [1, 365], x: 628, y: 338, dir: -60, c: '#6a3fb0' },
    { id: 12, n: 'Panorama loop above the lake', made: 0, km: 9.4, up: 520, t: 2, alt: 1128, p: 90, pn: 'Aussicht', asph: 18, f: 'loop views forest safe', snow: [day(5, 4), day(1, 12)], x: 588, y: 442, dir: 10, c: '#c8501e' },
    { id: 13, n: 'Moor and forest loop', made: 1, km: 11.1, up: 260, t: 1, alt: 1040, p: 45, pn: 'Moos', asph: 10, f: 'loop forest quiet safe', snow: [day(20, 4), day(21, 11)], x: 900, y: 290, dir: -90, c: '#2f7d4f' },
    { id: 14, n: 'Village and lake loop', made: 1, km: 7.6, up: 180, t: 1, alt: 520, p: 70, pn: 'Dorf', asph: 38, f: 'loop lake safe', snow: [1, 365], x: 648, y: 262, dir: 180, c: '#8c1d2b' },
    { id: 15, n: 'Valley floor loop', made: 1, km: 6.2, up: 120, t: 1, alt: 1080, p: 80, pn: 'Engelberg village', asph: 22, f: 'loop forest safe waterfall', snow: [day(1, 4), day(30, 11)], x: 601, y: 683, dir: 0, c: '#2f7d4f', img: 13 },
    { id: 16, n: 'Alp lakes loop', made: 0, km: 9.5, up: 480, t: 2, alt: 2050, p: 0, asph: 0, f: 'loop lake hut views safe', snow: [day(20, 6), day(15, 10)], x: 603, y: 640, dir: -90, c: '#2f62c9', img: 9 },
    { id: 17, n: 'Glacier view ridge loop', made: 0, km: 11.8, up: 940, t: 4, alt: 2450, p: 0, asph: 0, f: 'loop peak ridge views quiet', snow: [day(5, 7), day(30, 9)], x: 583, y: 725, dir: 100, c: '#0d6e74', img: 8 },
    { id: 18, n: 'Gorge walk, there and back', made: 0, km: 5.0, up: 210, t: 2, alt: 1210, p: 0, asph: 5, f: 'waterfall forest', snow: [day(15, 4), day(15, 11)], x: 612, y: 690, dir: 20, c: '#6a3fb0', img: 5 },
    // Routes with a lift for one half: the path runs from the valley station to the top station. `same` shares the line of another
    // record, because one path gives two shapes with different numbers: hike up and ride down, or ride up and hike down.
    { id: 19, n: 'Up to Alp Oberberg, cable car down', made: 1, shape: 'liftDown', lr: 'l1', km: 5.4, up: 820, down: 30, t: 2, alt: 1650, p: 60, pn: 'Oberberg valley station', asph: 6, f: 'hut views', snow: [day(10, 5), day(5, 11)], x: 468, y: 418, to: [4, 8], c: '#5a7d1e', img: 3 },
    { id: 20, n: 'Cable car to Alp Oberberg, down on foot', made: 1, shape: 'liftUp', lr: 'l1', same: 19, km: 5.4, up: 30, down: 820, t: 2, alt: 1650, p: 0, asph: 6, f: 'hut views', snow: [day(10, 5), day(5, 11)], x: 468, y: 418, to: [4, 8], c: '#5a7d1e', img: 3 },
    { id: 21, n: 'Up to the alp lakes, gondola down', made: 1, shape: 'liftDown', lr: 'l2', km: 6.1, up: 760, down: 20, t: 2, alt: 1840, p: 0, asph: 3, f: 'lake views', snow: [day(5, 6), day(25, 10)], x: 600, y: 674, to: [16, 0], c: '#1e6f9a', img: 9 },
    { id: 22, n: 'Gondola to the alp lakes, down on foot', made: 1, shape: 'liftUp', lr: 'l2', same: 21, km: 6.1, up: 20, down: 760, t: 2, alt: 1840, p: 0, asph: 3, f: 'lake views', snow: [day(5, 6), day(25, 10)], x: 600, y: 674, to: [16, 0], c: '#1e6f9a', img: 9 },
    { id: 23, n: 'Glacier view path, cable car up', made: 0, shape: 'liftUp', lr: 'l3', km: 9.2, up: 60, down: 1380, t: 3, alt: 2420, p: 0, asph: 0, f: 'views ridge waterfall quiet', snow: [day(1, 7), day(30, 9)], x: 590, y: 688, to: [17, 0], c: '#8c1d2b', img: 8 },
];
// The shapes a rider can accept. The lift shapes count only while Lifts is ticked.
const SHAPES = {
    loop: { label: 'Loop', tag: '' },
    back: { label: 'There and back', tag: 'There and back' },
    liftDown: { label: 'Hike up, lift down', tag: 'Lift down', lift: true },
    liftUp: { label: 'Lift up, hike down', tag: 'Lift up', lift: true },
};
const HIKE = Object.fromEntries(HIKES.map(h => [h.id, h]));
const LIFTS = [
    { id: 'l1', name: 'Cable car to Alp Oberberg', short: 'Cable car', valley: { x: 468, y: 418 }, on: [4, 8] },
    { id: 'l2', name: 'Gondola to the alp lakes', short: 'Gondola', valley: { x: 600, y: 674 }, on: [16, 0] },
    { id: 'l3', name: 'Cable car to the glacier', short: 'Cable car', valley: { x: 590, y: 688 }, on: [17, 0] },
];
const PLACES = [
    { id: 'chalch', kind: 'mountain', name: 'Chalchstock', alt: 1898, on: [1, 5] },
    { id: 'grat', kind: 'mountain', name: 'Gratspitz', alt: 2120, on: [4, 5] },
    { id: 'oberberg', kind: 'hut', name: 'Oberberg hut', on: [4, 2] },
    { id: 'hohgrat', kind: 'mountain', name: 'Hoh Grat', alt: 2404, on: [8, 5] },
    { id: 'gruensee', kind: 'lake', name: 'Grünsee', on: [9, 4] },
    { id: 'seeli', kind: 'lake', name: 'Seeli', on: [16, 6] },
    { id: 'stafel', kind: 'hut', name: 'Stafel hut', on: [16, 3] },
    { id: 'wissberg', kind: 'mountain', name: 'Wissberg', alt: 2450, on: [17, 5] },
    { id: 'alpli', kind: 'hut', name: 'Älpli hut', on: [3, 6] },
];
const PLACE = Object.fromEntries(PLACES.map(p => [p.id, p])), LIFT = Object.fromEntries(LIFTS.map(l => [l.id, l]));
const WISHES = [
    { key: 'peak', label: 'Summit', group: 'way', icon: 'mountain', place: 'mountain' },
    { key: 'ridge', label: 'Ridge', group: 'way', icon: 'ridge' },
    { key: 'lake', label: 'Lake to swim in', group: 'way', icon: 'lake', place: 'lake' },
    { key: 'hut', label: 'Hut with food', group: 'way', icon: 'hut', place: 'hut' },
    { key: 'waterfall', label: 'Waterfall', group: 'way', icon: 'waterfall' },
    { key: 'views', label: 'Wide views', group: 'way', icon: 'eye' },
    { key: 'official', label: 'Official route', group: 'route', icon: 'flag' },
    { key: 'safe', label: 'No steep ground near the path', group: 'route', icon: 'slope' },
    { key: 'nosnow', label: 'Usually free of snow on that day', short: 'Free of snow', group: 'route', icon: 'snow' },
    { key: 'forest', label: 'Mostly forest', group: 'route', icon: 'tree' },
    { key: 'quiet', label: 'Far from big roads', group: 'route', icon: 'quiet' },
];
const WISH = Object.fromEntries(WISHES.map(w => [w.key, w]));
const GRADES = [
    ['T1', 'Good path', 'flat or a gentle slope', 'a wide, even path'], ['T2', 'Mountain path', 'clear, with some steep parts', 'a narrow path on a slope'],
    ['T3', 'Rocky path', 'a chain in places, hands for balance', 'a rocky path with a chain'], ['T4', 'Alpine', 'the path is missing in places, hands needed', 'steep grass and rock'],
    ['T5', 'Hard alpine', 'often no path, easy climbing', 'a rock step'], ['T6', 'Climbing', 'mostly no path', 'an exposed rock ridge'],
];
const KM = { step: 2, steps: 15, unit: 'km' }, UP = { step: 100, steps: 15, unit: 'm' };
for (const h of HIKES) {
    h.min = Math.round((h.km / 4.2 + h.up / 450 + (h.down || 0) / 900) * 12) * 5;
    h.time = Math.floor(h.min / 60) + ' h' + (h.min % 60 ? ' ' + String(h.min % 60).padStart(2, '0') : '');
    h.loop = h.f.split(' ').includes('loop');
    h.shape = h.shape || (h.loop ? 'loop' : 'back');
}

// ── State ───────────────────────────────────────────────────────────────────
const fresh = () => ({ date: day(21, 6), point: null, walk: 4, car: false, road: 30, large: false, lift: 0, pin: null,
    km: [3, 8], up: [0, 15], loop: true, back: true, liftDown: true, liftUp: true, gradeMin: 1, gradeMax: 3, sort: 'match', wish: { peak: 1, hut: 1 }, selected: null, hover: null });
const STORE = 'explore-mock-3';
let state = fresh();
try { const saved = JSON.parse(localStorage.getItem(STORE) || 'null'); if (saved && saved.wish) state = { ...fresh(), ...saved, hover: null }; } catch (e) { /* The mock works without storage. */ }
const QUERY_KEYS = ['date', 'point', 'walk', 'car', 'road', 'large', 'lift', 'pin', 'km', 'up', 'loop', 'back', 'liftDown', 'liftUp', 'gradeMin', 'gradeMax', 'wish'];
const isFresh = () => { const f = fresh(); return QUERY_KEYS.every(k => JSON.stringify(f[k]) === JSON.stringify(state[k])); };
let picking = false;
const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;

// ── Lines of the sample hikes, measured once ────────────────────────────────
function rand(seed) { let x = seed * 9301 + 49297; return () => (x = (x * 9301 + 49297) % 233280) / 233280; }
function linePath(h) {
    if (h.same) { h.pts = HIKE[h.same].pts; return HIKE[h.same].d; }
    const r = rand(h.id), pts = [];
    if (h.to) {
        const end = HIKE[h.to[0]].pts[h.to[1]], len = Math.hypot(end[0] - h.x, end[1] - h.y), a0 = Math.atan2(end[1] - h.y, end[0] - h.x), n = 7;
        for (let i = 0; i < n; i++) { const d = i / (n - 1) * len, side = i && i < n - 1 ? (r() - 0.5) * 14 : 0; pts.push([h.x + Math.cos(a0) * d - Math.sin(a0) * side, h.y + Math.sin(a0) * d + Math.cos(a0) * side]); }
    } else if (h.loop) {
        const a0 = h.dir * Math.PI / 180, R = h.km * PX_KM / (2 * Math.PI) * 0.95, cx = h.x + Math.cos(a0) * R, cy = h.y + Math.sin(a0) * R, n = 11;
        for (let i = 0; i < n; i++) { const a = a0 + Math.PI + i / n * 2 * Math.PI, rr = i ? R * (0.72 + r() * 0.55) : R; pts.push([cx + Math.cos(a) * rr, cy + Math.sin(a) * rr]); }
    } else {
        const a0 = h.dir * Math.PI / 180, len = h.km / 2 * PX_KM, n = 7;
        for (let i = 0; i < n; i++) { const d = i / (n - 1) * len, side = i ? (r() - 0.5) * 14 : 0; pts.push([h.x + Math.cos(a0) * d - Math.sin(a0) * side, h.y + Math.sin(a0) * d + Math.cos(a0) * side]); }
    }
    h.pts = pts;
    const at = i => h.loop ? pts[(i + pts.length) % pts.length] : pts[Math.max(0, Math.min(pts.length - 1, i))];
    let d = `M${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)}`;
    for (let i = 0; i < (h.loop ? pts.length : pts.length - 1); i++) {
        const [p0, p1, p2, p3] = [at(i - 1), at(i), at(i + 1), at(i + 2)];
        d += `C${(p1[0] + (p2[0] - p0[0]) / 6).toFixed(1)} ${(p1[1] + (p2[1] - p0[1]) / 6).toFixed(1)} ${(p2[0] - (p3[0] - p1[0]) / 6).toFixed(1)} ${(p2[1] - (p3[1] - p1[1]) / 6).toFixed(1)} ${p2[0].toFixed(1)} ${p2[1].toFixed(1)}`;
    }
    return d + (h.loop ? 'Z' : '');
}
function profilePath(h) {
    const r = rand(h.id + 40), n = 28, ys = [];
    for (let i = 0; i <= n; i++) { const t = i / n, hill = Math.sin(Math.PI * Math.pow(t, h.loop ? 0.8 : 1)); ys.push(hill * (0.82 + r() * 0.18) + (i % 5 === 2 ? r() * 0.1 : 0)); }
    const line = ys.map((y, i) => `${i ? 'L' : 'M'}${(i / n * 300).toFixed(1)} ${(50 - y * 42).toFixed(1)}`).join('');
    return { line, area: line + 'L300 54L0 54Z' };
}
{
    const measure = document.createElementNS(NS, 'svg');
    measure.setAttribute('width', '0'); measure.setAttribute('height', '0'); measure.style.position = 'absolute';
    document.body.append(measure);
    for (const h of HIKES) {
        h.d = linePath(h);
        const el = document.createElementNS(NS, 'path'); el.setAttribute('d', h.d); measure.append(el);
        const L = el.getTotalLength(); h.samples = [];
        for (let s = 0; s <= L; s += 2.5) { const p = el.getPointAtLength(s); h.samples.push([p.x, p.y]); }
    }
    measure.remove();
}
const km = (a, b) => Math.hypot(a.x - b.x, a.y - b.y) / PX_KM;
const roadKm = (a, b) => Math.max(1, Math.round(km(a, b) * ROAD));
// The point of a route nearest to p, and its distance in km.
const nearest = (h, p) => { let best = [Infinity, 0, 0]; for (const [x, y] of h.samples) { const d = Math.hypot(x - p.x, y - p.y); if (d < best[0]) best = [d, x, y]; } return { km: best[0] / PX_KM, x: best[1], y: best[2] }; };
const toRoute = (h, p) => nearest(h, p).km;
const pointOn = ([id, i]) => { const [x, y] = HIKE[id].pts[i]; return { x, y }; };
const PARKS = HIKES.filter(h => h.p).map(h => ({ id: 'p' + h.id, name: 'Car park ' + h.pn, short: h.pn, x: h.x, y: h.y, spaces: h.p }));
const PARK = Object.fromEntries(PARKS.map(p => [p.id, p]));
for (const l of LIFTS) l.top = pointOn(l.on);
for (const p of PLACES) Object.assign(p, pointOn(p.on));
// What each hike can be reached from, and the places it passes. The bake would store this.
for (const h of HIKES) {
    h.parks = PARKS.map(park => ({ park, walk: toRoute(h, park) })).filter(n => n.walk <= 5);
    h.lifts = LIFTS.map(lift => ({ lift, walk: toRoute(h, lift.top) })).filter(n => n.walk <= 5);
    h.via = new Set(PLACES.filter(p => toRoute(h, p) <= 0.35).map(p => p.id));
}
const fromCache = new Map();
const walkFrom = (h, o) => { const key = h.id + '@' + o.x + ',' + o.y; if (!fromCache.has(key)) fromCache.set(key, toRoute(h, o)); return fromCache.get(key); };
const pinned = (s = state) => s.pin ? (s.pin.kind === 'park' ? PARK[s.pin.id] : LIFT[s.pin.id]) : null;
const pinName = (s = state, inline) => { const p = pinned(s); return !p ? '' : s.pin.kind === 'park' ? p.name : (inline ? 'the top' : 'Top') + ' of the ' + p.name.toLowerCase(); };
const pinAt = (s = state) => { const p = pinned(s); return p && (s.pin.kind === 'park' ? p : p.top); };

// ── Search ──────────────────────────────────────────────────────────────────
// All the ways to reach a hike, easiest first. With no start point, every hike counts. Every way ends with a walk of at most the walk limit.
function access(h, s = state) {
    const W = WALK[s.walk], out = [];
    if (s.pin) {
        if (s.pin.kind === 'park') { const n = h.parks.find(n => n.park.id === s.pin.id); if (n && n.walk <= W) out.push({ kind: 'park', park: n.park, walk: n.walk, cost: n.walk * 4 }); }
        else { const n = h.lifts.find(n => n.lift.id === s.pin.id); if (n && n.walk <= W) out.push({ kind: 'top', lift: n.lift, walk: n.walk, cost: n.walk * 4 }); }
        return out;
    }
    if (!s.point) return [{ kind: 'any', cost: 0 }];
    const o = s.point, onlyLift = s.lift === 2 && !h.lr;
    if (!onlyLift) {
        const d = walkFrom(h, o);
        if (d <= W) out.push({ kind: 'foot', walk: d, cost: d * 4 });
        if (s.car) for (const n of h.parks) {
            const road = roadKm(o, n.park);
            if (road <= s.road && (!s.large || isLarge(n.park)) && n.walk <= W) out.push({ kind: 'car', park: n.park, road, walk: n.walk, cost: road + n.walk * 4 });
        }
    }
    if (s.lift && !h.lr) for (const n of h.lifts) {
        if (n.walk > W) continue;
        const dv = km(o, n.lift.valley);
        if (dv <= W) out.push({ kind: 'lift', lift: n.lift, by: 'foot', before: dv, walk: n.walk, cost: (dv + n.walk) * 4 + 1 });
        else if (s.car) { const road = roadKm(o, n.lift.valley); if (road <= s.road) out.push({ kind: 'lift', lift: n.lift, by: 'car', road, walk: n.walk, cost: road + n.walk * 4 + 1 }); }
    }
    return out.sort((a, b) => a.cost - b.cost);
}
function has(h, key, s = state) {
    if (key === 'official') return !h.made;
    if (key === 'nosnow') return s.date >= h.snow[0] && s.date <= h.snow[1];
    if (key === 'reach') return h.t >= s.gradeMin;
    if (key.startsWith('via:')) return h.via.has(key.slice(4));
    return h.f.split(' ').includes(key);
}
const wishLabel = (k, s = state, long) => k === 'reach' ? `Has a T${s.gradeMin} part` : k.startsWith('via:') ? `Passes ${PLACE[k.slice(4)].name}` : (!long && WISH[k].short) || WISH[k].label;
const wishIcon = k => k === 'reach' ? 'up' : k.startsWith('via:') ? PLACE[k.slice(4)].kind : WISH[k].icon;
const shapeOk = (h, s = state) => s[h.shape] && (!SHAPES[h.shape].lift || !!s.lift);
const shapesOn = (s = state) => Object.keys(SHAPES).filter(k => s[k] && (!SHAPES[k].lift || s.lift));
const top = (range, scale) => range[1] >= scale.steps ? Infinity : range[1] * scale.step;
function inLimits(h, s, skip) {
    if (h.t > s.gradeMax || !shapeOk(h, s)) return false;
    if (skip !== 'km' && (h.km < s.km[0] * KM.step || h.km > top(s.km, KM))) return false;
    if (skip !== 'up' && (h.up < s.up[0] * UP.step || h.up > top(s.up, UP))) return false;
    return true;
}
const ORDER = {
    match: (a, b) => b.met.length - a.met.length || a.h.asph - b.h.asph || a.acc[0].cost - b.acc[0].cost,
    near: (a, b) => a.acc[0].cost - b.acc[0].cost || ORDER.match(a, b), short: (a, b) => a.h.km - b.h.km, climb: (a, b) => a.h.up - b.h.up, time: (a, b) => a.h.min - b.h.min,
};
function find(s = state, skip) {
    const keys = Object.keys(s.wish), must = keys.filter(k => s.wish[k] === 2), nice = keys.filter(k => s.wish[k] === 1), out = [];
    for (const h of HIKES) {
        if (!inLimits(h, s, skip) || !must.every(k => has(h, k, s))) continue;
        const acc = access(h, s);
        if (acc.length) out.push({ h, acc, met: nice.filter(k => has(h, k, s)), missed: nice.filter(k => !has(h, k, s)) });
    }
    return out.sort(ORDER[s.sort]);
}
const niceWishes = () => Object.keys(state.wish).filter(k => state.wish[k] === 1);
const rangeText = (range, scale) => {
    const low = range[0] * scale.step, high = top(range, scale);
    if (!low && high === Infinity) return 'Any';
    if (high === Infinity) return `At least ${fmt(low)} ${scale.unit}`;
    return low ? `${fmt(low)} to ${fmt(high)} ${scale.unit}` : `Up to ${fmt(high)} ${scale.unit}`;
};

// ── How a hike is reached, as text ──────────────────────────────────────────
const parkText = p => `${isLarge(p) ? 'Large' : 'Small'} car park`;
const parkAt = h => h.parks.filter(n => n.walk <= 0.3).sort((x, y) => y.park.spaces - x.park.spaces)[0];
const liftAt = h => h.lifts.find(n => n.walk <= 0.3);
const pbadge = '<span class="pbadge" aria-hidden="true">P</span>';
function accessSteps(a, h) {
    const step = (ico, text) => `<span class="step">${ico}${text}</span>`;
    const walk = a.walk >= 0.15 ? step(icon('walk', 14), dist(a.walk)) : '';
    const park = p => step(pbadge, isLarge(p) ? 'large' : 'small');
    if (a.kind === 'any') {
        const n = parkAt(h), l = liftAt(h);
        return n ? step(pbadge, parkText(n.park)) : l ? step(icon('lift', 14), l.lift.short) : '<span class="step none">No car park</span>';
    }
    if (a.kind === 'foot') return step(icon('walk', 14), dist(a.walk));
    if (a.kind === 'car') return step(icon('car', 14), `${a.road} km`) + park(a.park) + walk;
    if (a.kind === 'park') return park(a.park) + (walk || step('', 'starts there'));
    if (a.kind === 'top') return step(icon('lift', 14), a.lift.short) + (walk || step('', 'starts there'));
    return (a.by === 'car' ? step(icon('car', 14), `${a.road} km`) : a.before >= 0.15 ? step(icon('walk', 14), dist(a.before)) : '') + step(icon('lift', 14), a.lift.short) + walk;
}
function accessText(a, h) {
    const walk = a.walk >= 0.15 ? `, then walk ${dist(a.walk)} to the route` : ', the route starts there';
    if (a.kind === 'any') { const n = parkAt(h), l = liftAt(h); return (n ? `${n.park.name} is at the start` : l ? `The ${l.lift.name.toLowerCase()} stops on the route` : 'No car park at the route') + '. Set a start point to see how you get there'; }
    if (a.kind === 'foot') return `Walk ${dist(a.walk)} from your start to the route`;
    if (a.kind === 'car') return `Drive ${a.road} km to ${a.park.name} (${parkText(a.park).toLowerCase()})${walk}`;
    if (a.kind === 'park') return `Start at ${a.park.name}${walk}`;
    if (a.kind === 'top') return `Start at the top of the ${a.lift.name.toLowerCase()}${walk}`;
    return `${a.by === 'car' ? `Drive ${a.road} km to` : `Walk ${dist(a.before)} to`} the ${a.lift.name.toLowerCase()}, ride up${walk}`;
}

// ── Render loop ─────────────────────────────────────────────────────────────
const syncs = [];
let page = { render() {} };
function render() {
    const found = find();
    if (state.selected !== null && !found.some(r => r.h.id === state.selected)) state.selected = null;
    if (state.hover !== null && !found.some(r => r.h.id === state.hover)) state.hover = null;
    for (const sync of syncs) sync(found);
    syncWishes(found);
    page.render(found);
    renderMap(found);
    try { const { hover, ...keep } = state; localStorage.setItem(STORE, JSON.stringify(keep)); } catch (e) { /* Not kept. */ }
}
const update = change => { Object.assign(state, change); render(); };
function setWish(key, value) { const wish = { ...state.wish }; if (value) wish[key] = value; else delete wish[key]; update({ wish }); }
function setGradeMin(min) { const wish = { ...state.wish }; if (min > 1) wish.reach = wish.reach || 1; else delete wish.reach; update({ gradeMin: min, wish }); }
const reset = () => { state = { ...fresh(), sort: state.sort }; picking = false; render(); };
const setPin = pin => update({ pin, selected: null });

// ── Controls ────────────────────────────────────────────────────────────────
const head = (title, extra = '') => `<div class="head"><h2>${title}</h2>${extra}<span class="val"></span></div>`;
const rail = (el, min, max, low, high) => { el.style.setProperty('--steps', max - min); el.style.setProperty('--low', low - min); el.style.setProperty('--high', high - min); };
const wishRow = k => `<div class="wish" data-key="${k}"><button type="button" class="pick" aria-pressed="false"><span class="box">${icon('check', 12)}</span><span class="ico">${icon(wishIcon(k))}</span><span class="lab"></span></button><button type="button" class="must" aria-pressed="false" hidden title="Only show hikes that have this">${icon('lock', 12)}Must</button><span class="n"></span></div>`;
function syncRows(box, keys) {
    for (const el of [...box.children]) if (!keys.includes(el.dataset.key)) el.remove();
    keys.forEach((k, i) => {
        let el = box.querySelector(`[data-key="${CSS.escape(k)}"]`);
        if (!el) { box.insertAdjacentHTML('beforeend', wishRow(k)); el = box.lastElementChild; if (!reduced) el.classList.add('added'); }
        if (box.children[i] !== el) box.insertBefore(el, box.children[i] || null);
    });
}
function syncWishes(found) {
    for (const el of document.querySelectorAll('.wish')) {
        const k = el.dataset.key, v = state.wish[k] || 0, n = v === 2 ? found.length : find({ ...state, wish: { ...state.wish, [k]: 2 } }).length;
        el.classList.toggle('on', v > 0);
        el.querySelector('.lab').textContent = wishLabel(k, state, true);
        el.querySelector('.pick').setAttribute('aria-pressed', v > 0);
        const must = el.querySelector('.must'); must.hidden = !v; must.setAttribute('aria-pressed', v === 2);
        const count = el.querySelector('.n'); count.textContent = n; count.title = `${countText(n)} inside your limits ${n === 1 ? 'has' : 'have'} this`;
    }
}
document.addEventListener('click', e => {
    const wish = e.target.closest('.wish'); if (!wish) return;
    const key = wish.dataset.key, v = state.wish[key] || 0;
    if (e.target.closest('.must')) setWish(key, v === 2 ? 1 : 2);
    else if (e.target.closest('.pick')) { if (key === 'reach') setGradeMin(1); else setWish(key, v ? 0 : 1); }
});

// A leg of the journey from the start point to the route. Its node sits on the line that joins the legs.
const leg = (way, label, note, body) => `<li class="leg" data-way="${way}"><button type="button" class="leg-head" data-toggle="${way}" aria-pressed="false"><span class="box node">${icon('check', 12)}</span>${icon(way)}<span class="lab">${label}<small>${note}</small></span><b class="val"></b></button><div class="leg-body">${body}</div></li>`;
const GROUPS = {
    day(el) {
        el.innerHTML = head('Day of the hike', `<label class="date">${icon('calendar', 16)}<select aria-label="Day of the hike">${DATES.map(d => `<option value="${d}">${d === TODAY ? 'Today, ' : ''}${dateText(d)}</option>`).join('')}</select></label>`);
        const sel = el.querySelector('select');
        sel.addEventListener('change', e => update({ date: +e.target.value }));
        return () => { sel.value = state.date; };
    },
    journey(el) {
        el.innerHTML = head('Getting there') + `<ol class="journey">
            <li class="leg start"></li>
            ${leg('car', 'Car', 'to a car park or a lift', `<div class="rail"><input type="range" min="1" max="${ROAD_MAX / ROAD_STEP}" step="1" data-k="road" aria-label="Longest drive, one way"></div><button type="button" class="option" data-large aria-pressed="false"><span class="box">${icon('check', 12)}</span>Large car parks only</button>`)}
            <li class="leg later" data-way="train"><div class="leg-head"><span class="box node"></span>${icon('train')}<span class="lab">Train<small>Later: needs timetables</small></span></div></li>
            ${leg('lift', 'Lifts', 'cable cars, gondolas, chairlifts', `<span class="seg" role="group" aria-label="How lifts count"><button type="button" data-lift="1">Can take me up</button><button type="button" data-lift="2">Only by lift</button></span><p class="hint">${icon('info', 14)}We do not know when a lift runs.</p>`)}
            <li class="leg walk" data-way="walk"><div class="leg-head"><span class="node" title="You always walk the last part"></span>${icon('walk')}<span class="lab">Walk<small></small></span><b class="val"></b></div>
                <div class="leg-body"><div class="rail"><input type="range" min="0" max="${WALK.length - 1}" step="1" data-k="walk" aria-label="Longest walk to the route"></div></div></li>
            <li class="leg end"><div class="leg-head"><span class="node ring"></span><span></span><span class="lab">The route</span></div></li>
        </ol>`;
        const q = s => el.querySelector(s), way = k => q(`[data-way="${k}"]`), j = q('.journey');
        el.addEventListener('click', e => {
            if (e.target.closest('[data-unpin]')) return setPin(null);
            if (e.target.closest('[data-large]')) return update({ large: !state.large });
            const t = e.target.closest('[data-toggle]'); if (t) return update(t.dataset.toggle === 'car' ? { car: !state.car } : { lift: state.lift ? 0 : 1 });
            const l = e.target.closest('[data-lift]'); if (l) return update({ lift: +l.dataset.lift });
            const s = e.target.closest('[data-start]'); if (!s) return;
            if (s.dataset.start === 'pick') { picking = !picking; render(); if (picking && matchMedia('(max-width: 760px)').matches) mapEl.scrollIntoView({ block: 'center', behavior: reduced ? 'auto' : 'smooth' }); }
            if (s.dataset.start === 'gps') update({ point: { ...GPS, gps: true }, pin: null });
            if (s.dataset.start === 'clear') update({ point: null });
        });
        el.addEventListener('input', e => { const k = e.target.dataset.k; if (k === 'walk') update({ walk: +e.target.value }); if (k === 'road') update({ road: e.target.value * ROAD_STEP }); });
        let startHtml = '';
        return () => {
            const p = pinned(), pt = state.point, onPark = state.pin?.kind === 'park';
            j.classList.toggle('alone', !p && !pt); j.classList.toggle('pinned', !!p);
            const where = (ico, name, note, acts) => `<div class="leg-head"><span class="node dot"></span>${ico}<span class="lab">${name}<small>${note}</small></span><span class="acts">${acts}</span></div>`;
            const html = p ? where(onPark ? pbadge : icon('lift'), pinName(), 'Pinned on the map', '<button type="button" class="text-button" data-unpin>Unpin</button>')
                : pt ? where(icon('pin'), 'Near ' + townNear(pt), pt.gps ? 'Your location' : 'Picked on the map', `<button type="button" class="text-button" data-start="pick">Move</button><button type="button" class="close" data-start="clear" aria-label="Remove the start point">${icon('close', 16)}</button>`)
                : `<p class="hint">No start point: every hike in the map view counts.</p><div class="start-buttons"><button type="button" class="button" data-start="pick" aria-pressed="false">${icon('pin', 16)}Pick on the map</button><button type="button" class="button" data-start="gps">${icon('locate', 16)}My location</button></div>`;
            if (html !== startHtml) { q('.start').innerHTML = html; startHtml = html; }
            q('.start [data-start="pick"]')?.setAttribute('aria-pressed', picking);
            way('walk').querySelector('small').textContent = p ? `From the ${onPark ? 'car park' : 'top station'} to the route` : state.car || state.lift ? 'The last part, to the route' : 'From your start to the route';
            way('walk').querySelector('.val').textContent = 'up to ' + dist(WALK[state.walk]);
            const walk = q('[data-k="walk"]'); walk.value = state.walk; rail(walk.parentElement, 0, WALK.length - 1, 0, state.walk);
            for (const [k, on] of [['car', state.car], ['lift', !!state.lift]]) {
                way(k).classList.toggle('on', on);
                way(k).querySelector('.leg-head').setAttribute('aria-pressed', on);
                way(k).querySelector('.leg-body').hidden = !on;
            }
            way('car').querySelector('.val').textContent = state.car ? `up to ${state.road} km` : '';
            const road = q('[data-k="road"]'); road.value = state.road / ROAD_STEP; rail(road.parentElement, 1, ROAD_MAX / ROAD_STEP, 1, state.road / ROAD_STEP);
            q('[data-large]').setAttribute('aria-pressed', state.large);
            way('lift').querySelector('.val').textContent = state.lift === 2 ? 'only by lift' : '';
            for (const b of el.querySelectorAll('[data-lift]')) b.setAttribute('aria-pressed', +b.dataset.lift === state.lift);
        };
    },
    km(el) { return rangeGroup(el, 'km', 'Length', KM, ['Shortest length', 'Longest length']); },
    up(el) { return rangeGroup(el, 'up', 'Climb', UP, ['Least climb', 'Most climb']); },
    shape(el) {
        const loopIco = '<svg class="i" width="20" height="20" viewBox="0 0 24 24" aria-hidden="true"><path d="M5 19c-3-6 1-14 8-14s9 8 4 12-9 2-12 2Z"/><circle cx="5" cy="19" r="1.7" fill="currentColor"/></svg>';
        const backIco = '<svg class="i" width="20" height="20" viewBox="0 0 24 24" aria-hidden="true"><path d="M5 19c3-3 3-8 7-9s5-3 7-5"/><circle cx="5" cy="19" r="1.7" fill="currentColor"/><circle cx="19" cy="5" r="1.7"/></svg>';
        // A lift shape: the walked half is solid, the ride is dotted, so the picture says which half the lift takes.
        const liftIco = up => `<svg class="i" width="20" height="20" viewBox="0 0 24 24" aria-hidden="true"><path d="M4 20c4-2 6-8 9-10s4-4 7-6"/><path d="M4 20 20 4" stroke-dasharray="1.5 2.5"/><path d="${up ? 'M15 4h5v5' : 'M4 15v5h5'}"/></svg>`;
        el.innerHTML = head('Shape') + `<div class="tiles">
            <button type="button" class="tile" data-shape="loop">${loopIco}Loop</button>
            <button type="button" class="tile" data-shape="back">${backIco}There and back</button>
            <button type="button" class="tile lift-shape" data-shape="liftDown">${liftIco(false)}Hike up, lift down</button>
            <button type="button" class="tile lift-shape" data-shape="liftUp">${liftIco(true)}Lift up, hike down</button></div>
            <p class="hint" data-lift-hint>${icon('lift', 14)}Tick Lifts in Getting there to also see hikes that ride a lift up or down.</p>`;
        el.addEventListener('click', e => {
            const t = e.target.closest('[data-shape]'); if (!t) return;
            const k = t.dataset.shape, next = { ...state, [k]: !state[k] };
            if (!shapesOn(next).length) return toast('Keep at least one shape.');
            update({ [k]: next[k] });
        });
        return () => {
            for (const t of el.querySelectorAll('[data-shape]')) { t.setAttribute('aria-pressed', state[t.dataset.shape]); t.hidden = SHAPES[t.dataset.shape].lift && !state.lift; }
            el.querySelector('[data-lift-hint]').hidden = !!state.lift;
            el.querySelector('.val').textContent = shapesOn().length === (state.lift ? 4 : 2) ? 'All' : '';
        };
    },
    grade: gradeGroup,
    way(el) {
        el.innerHTML = head('On the way') + `<p class="hint">Ticked wishes put the hikes that have them first. Must hides the others.</p><div class="wishes" data-via></div><div class="wishes">${WISHES.filter(w => w.group === 'way').map(w => wishRow(w.key)).join('')}</div><p class="hint">${icon('pin', 14)}Or click a summit, hut or lake on the map.</p>`;
        return () => syncRows(el.querySelector('[data-via]'), Object.keys(state.wish).filter(k => k.startsWith('via:')));
    },
    route(el) {
        el.innerHTML = head('The route') + `<div class="wishes">${WISHES.filter(w => w.group === 'route').map(w => wishRow(w.key)).join('')}</div>`;
        return () => {};
    },
};
function rangeGroup(el, id, title, scale, labels) {
    el.innerHTML = head(title) + `<div class="bars" aria-hidden="true">${'<span></span>'.repeat(scale.steps)}</div>
        <div class="rail"><input type="range" min="0" max="${scale.steps}" step="1" data-end="0" aria-label="${labels[0]}"><input type="range" min="0" max="${scale.steps}" step="1" data-end="1" aria-label="${labels[1]}"></div>`;
    const [low, high] = el.querySelectorAll('input');
    low.addEventListener('input', () => update({ [id]: [Math.min(+low.value, state[id][1] - 1), state[id][1]] }));
    high.addEventListener('input', () => update({ [id]: [state[id][0], Math.max(+high.value, state[id][0] + 1)] }));
    return () => {
        const range = state[id], counts = Array(scale.steps).fill(0);
        for (const r of find(state, id)) counts[Math.min(scale.steps - 1, Math.floor(r.h[id] / scale.step))]++;
        const max = Math.max(1, ...counts);
        [...el.querySelector('.bars').children].forEach((bar, i) => { bar.style.setProperty('--share', counts[i] / max); bar.className = (counts[i] ? '' : 'none ') + (i >= range[0] && i < range[1] ? 'on' : ''); });
        rail(el.querySelector('.rail'), 0, scale.steps, range[0], range[1]); low.value = range[0]; high.value = range[1];
        const text = rangeText(range, scale);
        el.querySelector('.val').textContent = text + (id === 'up' && text !== 'Any' ? ' up' : '');
    };
}
// The upper thumb of the grade is a limit; the lower thumb is the wish "has a part of at least this grade".
// The picture of a grade opens after a second of hover, at once on keyboard focus, and stays open after a tap.
function gradeGroup(el) {
    el.classList.add('grade-group');
    el.innerHTML = head('Path difficulty', `<button type="button" class="info" aria-expanded="false" aria-label="What the grades mean">${icon('info', 16)}</button>`) + `
        <div class="rail"><input type="range" min="0" max="5" step="1" class="wishful" aria-label="Should reach at least this grade"><input type="range" min="0" max="5" step="1" aria-label="Hardest grade allowed"></div>
        <div class="grade-labels">${GRADES.map((g, i) => `<button type="button" data-grade="${i}" style="left: ${i * 20}%" aria-label="${g[0]}, ${g[1]}">${g[0]}</button>`).join('')}</div>
        <div class="wishes" data-reach></div><div class="grade-pop" role="tooltip" hidden></div>`;
    const [low, high] = el.querySelectorAll('input'), labels = el.querySelector('.grade-labels'), pop = el.querySelector('.grade-pop'), info = el.querySelector('.info');
    low.addEventListener('input', () => setGradeMin(Math.min(+low.value + 1, state.gradeMax)));
    high.addEventListener('input', () => update({ gradeMax: Math.max(+high.value + 1, state.gradeMin) }));
    let open = null, timer = 0;
    const show = i => {
        const g = GRADES[i], b = labels.children[i];
        for (const x of labels.children) x.classList.toggle('showing', x === b);
        pop.innerHTML = `${gradeSketch(i)}<p><b>${g[0]} · ${g[1]}</b>${g[2][0].toUpperCase() + g[2].slice(1)}.</p><small>A photo of ${g[3]} comes here.</small>`;
        pop.style.setProperty('--arrow', (labels.offsetLeft + b.offsetLeft) + 'px');
        pop.classList.remove('above'); pop.style.bottom = ''; pop.style.top = (labels.offsetTop + labels.offsetHeight + 8) + 'px'; pop.hidden = false;
        const scroller = el.closest('.scroll'), limit = scroller && scroller.scrollHeight > scroller.clientHeight ? scroller.getBoundingClientRect().bottom : innerHeight;
        if (pop.getBoundingClientRect().bottom > limit) { pop.classList.add('above'); pop.style.top = ''; pop.style.bottom = (el.offsetHeight - el.querySelector('.rail').offsetTop + 6) + 'px'; }
    };
    const hide = () => { if (open !== null) return show(open); pop.hidden = true; for (const x of labels.children) x.classList.remove('showing'); info.setAttribute('aria-expanded', false); };
    labels.addEventListener('pointerover', e => { const b = e.target.closest('[data-grade]'); if (!b) return; clearTimeout(timer); if (!pop.hidden) show(+b.dataset.grade); else timer = setTimeout(() => show(+b.dataset.grade), 1000); });
    labels.addEventListener('pointerleave', () => { clearTimeout(timer); hide(); });
    labels.addEventListener('focusin', e => { if (e.target.matches(':focus-visible')) show(+e.target.dataset.grade); });
    labels.addEventListener('focusout', hide);
    labels.addEventListener('click', e => { const b = e.target.closest('[data-grade]'); if (!b) return; clearTimeout(timer); const i = +b.dataset.grade; open = open === i ? null : i; open === null ? hide() : show(i); });
    info.addEventListener('click', e => { e.stopPropagation(); open = open === null ? state.gradeMax - 1 : null; info.setAttribute('aria-expanded', open !== null); open === null ? hide() : show(open); });
    document.addEventListener('click', e => { if (open !== null && !el.contains(e.target)) { open = null; hide(); } });
    document.addEventListener('keydown', e => { if (e.key === 'Escape' && open !== null) { open = null; hide(); } });
    return () => {
        rail(el.querySelector('.rail'), 0, 5, state.gradeMin - 1, state.gradeMax - 1); low.value = state.gradeMin - 1; high.value = state.gradeMax - 1;
        for (const b of labels.children) b.classList.toggle('in', +b.dataset.grade >= state.gradeMin - 1 && +b.dataset.grade <= state.gradeMax - 1);
        const g = GRADES[state.gradeMax - 1];
        el.querySelector('.val').textContent = `Up to ${g[0]} · ${g[1]}`;
        syncRows(el.querySelector('[data-reach]'), state.gradeMin > 1 ? ['reach'] : []);
    };
}
function gradeSketch(i) {
    const t = Math.tan([7, 14, 24, 34, 44, 56][i] * Math.PI / 180), y = x => 62 - (x - 140) * t;
    const way = i < 4 ? `<path class="path" d="M-10 ${y(-10)}L290 ${y(290)}" stroke-width="${[9, 6, 3.5, 2.5][i]}" ${i > 1 ? `stroke-dasharray="${i === 2 ? '9 6' : '2 8'}"` : ''}/>` : '';
    const rocks = [[40, 7], [92, 5], [196, 8], [236, 6], [64, 9], [214, 10], [20, 8], [262, 9]].slice(0, [0, 0, 3, 5, 7, 8][i]).map(([x, r]) => `<path class="rock" d="M${x - r} ${y(x - r) + 1}L${x - r * 0.2} ${y(x) - r * (1 + i * 0.25)}L${x + r} ${y(x + r) + 1}Z"/>`).join('');
    const chain = i === 2 ? `<path class="chain" d="M150 ${y(150) - 13}Q175 ${y(175) - 6} 200 ${y(200) - 13}Q225 ${y(225) - 6} 250 ${y(250) - 13}M150 ${y(150)}v-13M200 ${y(200)}v-13M250 ${y(250)}v-13"/>` : '';
    return `<svg viewBox="0 0 280 112" aria-hidden="true"><rect class="sky" width="280" height="112"/><path class="hill" d="M-10 ${y(-10)}L290 ${y(290)}V130H-10Z"/>${way}${rocks}${chain}
        <g class="walker" transform="translate(${118 - i * 1.5} ${y(128) - 31}) scale(1.35)"><path d="M15 4a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Zm-3 3 4 4h3M13 6l-3 7 5 3-1 6m-4-9-3 5-4 3M11 7l-3-1-2 6 3 1m11-3-2 12"/></g></svg>`;
}
function mountFilters(el) {
    for (const name of ['day', 'journey', 'km', 'up', 'shape', 'grade', 'way', 'route']) {
        const g = document.createElement('section'); g.className = 'group'; g.dataset.group = name; el.append(g);
        syncs.push(GROUPS[name](g));
    }
}

// ── The guide: the same filter groups, one question at a time ──────────────
// The guide mounts its own copy of each group, so an answer sets the same state as the filter column, and the map and the
// results change while the rider answers. A rider can skip any question, go back, or close the guide at any step.
const STEPS = [
    { groups: ['day'], q: 'When do you go?', why: 'The day decides which hikes are usually free of snow.' },
    { groups: ['journey'], q: 'How do you get to the start?', why: 'Set where you start, then tick each way you are happy to travel.' },
    { groups: ['shape'], q: 'Which shapes are fine?', why: 'Pick every shape you would walk. A there-and-back hike counts its length and climb both ways.' },
    { groups: ['km', 'up'], q: 'How long, and how much climb?', why: 'The bars show how many hikes each step holds, inside your other answers.' },
    { groups: ['grade'], q: 'How hard may the path be?', why: 'The right handle is the hardest grade you accept. Move the left handle to ask for a harder part. Hover or tap a grade to see it.' },
    { groups: ['way'], q: 'What would make it a good day?', why: 'Pick what you hope to pass on the way.' },
    { groups: ['route'], q: 'Anything about the route itself?', why: 'For example, a route that is usually free of snow on your day.' },
];
function mountGuide(side, filters, opener) {
    const g = document.createElement('section');
    g.className = 'guide'; g.hidden = true; g.setAttribute('aria-label', 'Plan your hike, one question at a time');
    g.innerHTML = `<div class="guide-top"><span class="guide-step" aria-live="polite"></span><span class="guide-dots" aria-hidden="true">${STEPS.map(() => '<i></i>').join('')}</span><button type="button" class="close" data-guide-close aria-label="Close the guide">${icon('close', 16)}</button></div>
        <div class="guide-body scroll">${STEPS.map((st, i) => `<div class="guide-page" data-page="${i}" hidden><h3>${st.q}</h3><p class="hint">${st.why}</p><div class="filters"></div></div>`).join('')}</div>
        <div class="guide-foot"><button type="button" class="text-button" data-guide-back>Back</button><span class="guide-count"></span><button type="button" class="text-button" data-guide-skip>Skip</button><button type="button" class="button primary" data-guide-next></button></div>`;
    side.append(g);
    STEPS.forEach((st, i) => {
        const box = g.querySelector(`[data-page="${i}"] .filters`);
        for (const name of st.groups) { const el = document.createElement('section'); el.className = 'group'; el.dataset.group = name; box.append(el); syncs.push(GROUPS[name](el)); }
    });
    // The offer at the top of the column, while the filters hold nothing of the rider's own.
    const offer = document.createElement('div');
    offer.className = 'guide-offer';
    offer.innerHTML = `<p><b>New search?</b> Seven short questions set the filters for you. Skip any of them.</p><button type="button" class="button small" data-guide-open>${icon('walk', 14)}Guide me</button>`;
    filters.prepend(offer);
    let step = -1;
    const show = i => {
        step = i; g.hidden = i < 0; side.classList.toggle('guided', i >= 0);
        if (i < 0) return opener.focus();
        for (const p of g.querySelectorAll('.guide-page')) p.hidden = +p.dataset.page !== i;
        [...g.querySelector('.guide-dots').children].forEach((dot, j) => dot.className = j < i ? 'done' : j === i ? 'now' : '');
        g.querySelector('.guide-step').textContent = `Question ${i + 1} of ${STEPS.length}`;
        g.querySelector('[data-guide-back]').disabled = i === 0;
        g.querySelector('[data-guide-skip]').hidden = i === STEPS.length - 1;
        g.querySelector('.guide-body').scrollTop = 0;
        sync(find());
        g.querySelector('h3:not([hidden])') && g.querySelector(`[data-page="${i}"] h3`).focus({ preventScroll: true });
    };
    const sync = found => {
        offer.hidden = !isFresh();
        if (step < 0) return;
        g.querySelector('.guide-count').textContent = `${countText(found.length)} fit so far`;
        g.querySelector('[data-guide-next]').textContent = step === STEPS.length - 1 ? `Show ${countText(found.length)}` : 'Next';
    };
    for (const h of g.querySelectorAll('h3')) h.tabIndex = -1;
    syncs.push(sync);
    document.addEventListener('click', e => {
        if (e.target.closest('[data-guide-open]')) return show(0);
        if (step < 0 || !g.contains(e.target)) return;
        if (e.target.closest('[data-guide-close]')) return show(-1);
        if (e.target.closest('[data-guide-back]')) return show(Math.max(0, step - 1));
        if (e.target.closest('[data-guide-skip]')) return show(step + 1);
        if (e.target.closest('[data-guide-next]')) return show(step === STEPS.length - 1 ? -1 : step + 1);
    });
    g.addEventListener('keydown', e => { if (e.key === 'Escape' && !e.defaultPrevented) show(-1); });
    return { open: () => show(0) };
}

// ── One plain line that says what the search holds ─────────────────────────
function queryHtml() {
    const parts = [];
    if (state.pin) parts.push(`From <b>${pinName(state, true)}</b>, walk ${dist(WALK[state.walk])}`);
    else if (state.point) parts.push(`From <b>near ${townNear(state.point)}</b>: ` + [`walk ${dist(WALK[state.walk])}`, state.car && `drive ${state.road} km`, state.lift === 2 ? 'by lift only' : state.lift && 'lifts'].filter(Boolean).join(', '));
    else parts.push('In the map view');
    const shapes = shapesOn();
    if (shapes.length < (state.lift ? 4 : 2)) parts.push(shapes.map(k => SHAPES[k].label.toLowerCase()).join(' or '));
    if (state.km[0] > 0 || state.km[1] < 15) parts.push(rangeText(state.km, KM).toLowerCase());
    if (state.up[0] > 0 || state.up[1] < 15) parts.push(rangeText(state.up, UP).toLowerCase() + ' up');
    parts.push('up to T' + state.gradeMax);
    const musts = Object.keys(state.wish).filter(k => state.wish[k] === 2).map(k => `<b>${wishLabel(k).toLowerCase()}</b>`);
    return parts.join(' · ') + (musts.length ? ` · must: ${musts.join(', ')}` : '') + '.';
}
const countText = n => `${n} ${n === 1 ? 'hike' : 'hikes'}`;

// ── Hover, the empty list and the details of a hike ─────────────────────────
function hover(id) {
    if (state.hover === id) return;
    state.hover = id;
    renderMap(find());
    for (const el of document.querySelectorAll('tr[data-id]')) el.toggleAttribute('data-hover', +el.dataset.id === id);
}
// The empty list names what blocks the search, with the count each change would give.
let emptyTries = [];
function emptyHtml() {
    const tries = [];
    for (const k of Object.keys(state.wish).filter(k => state.wish[k] === 2)) tries.push([`${wishLabel(k)} as a wish only`, { wish: { ...state.wish, [k]: 1 } }]);
    if (state.lift === 2) tries.push(['Not only by lift', { lift: 1 }]);
    if (state.walk < WALK.length - 1) tries.push([`Walk up to ${dist(WALK[state.walk + 2] || WALK[WALK.length - 1])}`, { walk: Math.min(WALK.length - 1, state.walk + 2) }]);
    if (state.point && !state.pin) {
        if (state.car && state.road < ROAD_MAX) tries.push([`Drive up to ${Math.min(ROAD_MAX, state.road + 20)} km`, { road: Math.min(ROAD_MAX, state.road + 20) }]);
        if (state.large) tries.push(['Small car parks too', { large: false }]);
        if (!state.car) tries.push(['Take the car', { car: true }]);
        if (!state.lift) tries.push(['Take lifts', { lift: 1 }]);
    }
    if (state.pin) tries.push(['Unpin the start', { pin: null }]);
    for (const k of Object.keys(SHAPES)) if (!state[k] && (!SHAPES[k].lift || state.lift)) tries.push([`${SHAPES[k].label} too`, { [k]: true }]);
    const ways = tries.map(([text, change]) => [text, change, find({ ...state, ...change }).length]).filter(t => t[2]);
    emptyTries = ways.map(t => t[1]);
    return `<div class="empty"><strong>No hike fits all of this.</strong>${ways.length ? `<span>One change would help:</span><div>${ways.map(([text, , n], i) => `<button type="button" class="button small" data-try="${i}">${text}<span class="n">${countText(n)}</span></button>`).join('')}</div>` : '<span>Widen the length or the climb.</span>'}<button type="button" class="text-button" data-reset>Reset the filters</button></div>`;
}
const tagsHtml = r => [...r.met.map(k => `<span class="tag">${icon('check', 12)}${wishLabel(k)}</span>`), ...r.missed.map(k => `<span class="tag no">${icon('close', 11)}${wishLabel(k)}</span>`)].join('');
// What the shape means for the rider, with the numbers that the table does not show.
function shapeText(h) {
    const lift = h.lr && LIFT[h.lr].short.toLowerCase();
    if (h.shape === 'back') return `There and back, ${fmt(Math.round(h.km / 2 * 10) / 10)} km each way; length and climb count both ways`;
    if (h.shape === 'liftDown') return `Hike up ${fmt(h.up)} m, then the ${lift} down; park at the valley station`;
    if (h.shape === 'liftUp') return `The ${lift} up, then hike down ${fmt(h.down)} m; park at the valley station`;
    return 'Loop';
}
function detailHtml(r) {
    const h = r.h, prof = profilePath(h), safe = has(h, 'safe'), year = h.snow[0] === 1 && h.snow[1] === 365, a = r.acc[0];
    const trail = a.park || a.lift ? a : !state.pin && a.kind === 'any' ? (parkAt(h) && { park: parkAt(h).park }) || (liftAt(h) && { lift: liftAt(h).lift }) : null;
    const pinFrom = !state.pin && trail ? (trail.park ? { kind: 'park', id: trail.park.id } : { kind: 'lift', id: trail.lift.id }) : null;
    return `<div class="more-box">
        <div class="cover"><img src="img/h${h.img || h.id}.webp" alt="" loading="lazy" onerror="this.hidden=true"></div>
        <div class="col">
            <p class="kind">${shapeText(h)} · ${h.made ? `Our ${h.loop ? 'loop' : 'route'}, made from signed hiking paths` : 'Official hiking route'}</p>
            <p class="how">${accessText(a, h)}.</p>
            ${pinFrom ? `<button type="button" class="button small" data-pin='${JSON.stringify(pinFrom)}'>${icon('pin', 14)}Hikes from this ${pinFrom.kind === 'park' ? 'car park' : 'top station'}</button>` : ''}
            ${r.met.length + r.missed.length ? `<div class="tags">${tagsHtml(r)}</div>` : ''}
            <svg class="profile" viewBox="0 0 300 54" preserveAspectRatio="none" aria-hidden="true"><path class="area" d="${prof.area}"/><path class="top" d="${prof.line}"/></svg>
        </div>
        <div class="col">
            <ul class="facts">
                <li>${icon('mountain', 16)}Highest point ${fmt(h.alt)} m</li>
                <li>${icon('slope', 16)}${safe ? 'No steep ground near the path' : 'Steep ground beside the path'}</li>
                <li>${icon('snow', 16)}${year ? 'Usually free of snow all year' : `Usually free of snow ${dateText(h.snow[0], true)} to ${dateText(h.snow[1], true)}`}</li>
                <li>${icon('asph', 16)}${h.asph} % asphalt</li>
                ${h.via.size ? `<li>${icon('pin', 16)}Passes ${[...h.via].map(id => PLACE[id].name).join(', ')}</li>` : ''}
            </ul>
            <div class="actions"><button type="button" class="button primary" data-still>Plan this route</button><button type="button" class="text-button" data-close>Close</button></div>
        </div>
    </div>`;
}
document.addEventListener('click', e => {
    const t = e.target.closest('[data-try]'); if (t) return update(emptyTries[+t.dataset.try]);
    if (e.target.closest('[data-reset]')) return reset();
    if (e.target.closest('[data-close]')) return update({ selected: null });
    const pin = e.target.closest('[data-pin]'); if (pin) return setPin(JSON.parse(pin.dataset.pin));
    if (e.target.closest('[data-still]')) toast('Not part of this mock.');
});
function toast(text) {
    document.querySelector('.toast')?.remove();
    const t = document.createElement('p'); t.className = 'toast'; t.setAttribute('role', 'status'); t.textContent = text;
    document.body.append(t); setTimeout(() => t.remove(), 2200);
}
document.addEventListener('keydown', e => {
    if (e.key !== 'Escape') return;
    if (picking) { picking = false; return render(); }
    if (state.selected !== null) update({ selected: null });
});

// ── The map ─────────────────────────────────────────────────────────────────
// Quiet markers: car parks and lifts the results use show only when the view is closer than FAR map units; places of a ticked wish show smaller when far.
// The hovered or selected hike's trailhead and places always show, with names; a pinned start and a "Passes" place carry a flag with a remove button.
const FAR = 620;
let mapEl = null, svg = null, tip = null, flagsEl = null, flagsKey = '';
const vb = { x: 0, y: 0, w: MAP_W, h: MAP_H };
let fitKey = null, fitAnim = 0, fitted = false, settle = 0;
const marker = (cls, attrs, x, y, glyph, label) => `<g class="mark ${cls}" ${attrs} transform="translate(${x.toFixed(1)} ${y.toFixed(1)})"><g class="sz"><g class="mk">${glyph}</g><text x="13" y="4">${label}</text></g></g>`;
function mountMap(el) {
    mapEl = el;
    el.innerHTML = `<svg viewBox="0 0 ${MAP_W} ${MAP_H}" preserveAspectRatio="xMidYMid slice" role="img" aria-label="Map of the hikes in the list">
            <defs><mask id="reach-mask"><rect x="-2000" y="-2000" width="6000" height="6000" fill="#fff"/><circle class="hole" r="1" fill="#000"/></mask></defs>
            <rect width="${MAP_W}" height="${MAP_H}" fill="#e9e6da"/><image class="picture" href="img/map.jpg" width="${MAP_W}" height="${MAP_H}"/>
            <rect class="dim" x="-2000" y="-2000" width="6000" height="6000" mask="url(#reach-mask)"/>
            <circle class="reach" r="1"/><path class="approach"/>
            <g>${HIKES.map(h => `<g class="hike" data-hike="${h.id}"><path class="casing" d="${h.d}"/><path class="line" d="${h.d}" stroke="${h.c}"/><path class="hit" d="${h.d}"/></g>`).join('')}</g>
            <g>${LIFTS.map(l => `<g class="lift" data-lift="${l.id}"><path class="cable" d="M${l.valley.x} ${l.valley.y}L${l.top.x.toFixed(1)} ${l.top.y.toFixed(1)}"/><circle class="valley" cx="${l.valley.x}" cy="${l.valley.y}" r="3.5"/>
                ${marker('top', '', l.top.x, l.top.y, `<circle r="9"/><path transform="translate(-6 -6) scale(.5)" d="${ICON.lift}"/>`, l.short)}</g>`).join('')}</g>
            <g>${PARKS.map(p => marker('park', `data-park="${p.id}"`, p.x, p.y, '<rect x="-9" y="-9" width="18" height="18" rx="4"/><text class="p" y="3.6">P</text>', p.short)).join('')}</g>
            <g>${PLACES.map(p => marker('poi', `data-place="${p.id}"`, p.x, p.y, `<circle r="8.5"/><path transform="translate(-5.5 -5.5) scale(.46)" d="${ICON[p.kind]}"/>`, p.name + (p.alt ? ' ' + fmt(p.alt) : ''))).join('')}</g>
            <g class="you" hidden><g class="sz"><circle r="7"/><text x="11" y="4"></text></g></g>
        </svg>
        <div class="flags"></div>
        <div class="map-buttons"><div class="ctrl"><button type="button" class="trailview" data-still>${icon('binoculars')}TrailView</button></div><div class="ctrl"><button type="button" aria-label="Zoom in" data-still>${icon('plus', 16)}</button><button type="button" aria-label="Zoom out" data-still>${icon('minus', 16)}</button></div></div>
        <span class="credit">© OpenStreetMap contributors</span>
        <span class="pick-banner" hidden>Click the map where you start. Esc cancels.</span>`;
    svg = el.querySelector('svg'); flagsEl = el.querySelector('.flags');
    tip = document.createElement('div'); tip.className = 'tip'; tip.hidden = true; el.append(tip);
    const showTip = (x, y, html) => { const [sx, sy] = toScreen(x, y); tip.innerHTML = html; tip.style.left = Math.max(110, Math.min(el.clientWidth - 110, sx)) + 'px'; tip.style.top = Math.max(64, sy) + 'px'; tip.hidden = false; };
    svg.addEventListener('pointerover', e => {
        if (picking) return;
        const hike = e.target.closest('.hike'), park = e.target.closest('.park'), lift = e.target.closest('.top'), poi = e.target.closest('.poi');
        if (park || lift) {
            const pin = park ? { kind: 'park', id: park.dataset.park } : { kind: 'lift', id: lift.closest('.lift').dataset.lift }, on = state.pin?.id === pin.id;
            const p = park ? PARK[pin.id] : LIFT[pin.id], n = find({ ...state, pin }).length, at = park ? p : p.top;
            return showTip(at.x, at.y, `<b>${p.name}</b>${park ? parkText(p) : 'Top station'} · ${countText(n)} within a ${dist(WALK[state.walk])} walk<em>${on ? 'Click: unpin' : 'Click: only hikes from here'}</em>`);
        }
        if (poi) {
            const p = PLACE[poi.dataset.place], k = 'via:' + p.id, on = state.wish[k], n = find({ ...state, wish: { ...state.wish, [k]: 2 } }).length;
            return showTip(p.x, p.y, `<b>${p.name}${p.alt ? ' ' + fmt(p.alt) + ' m' : ''}</b>${countText(n)} ${n === 1 ? 'passes' : 'pass'} here<em>${on ? 'Click: show all hikes again' : 'Click: only hikes that pass here'}</em>`);
        }
        if (hike) { const h = HIKE[+hike.dataset.hike]; hover(h.id); const [x, y] = h.pts[0]; showTip(x, y, `<b>${h.n}</b>${h.km} km · ${fmt(h.up)} m up · T${h.t}`); page.reveal?.(h.id); }
    });
    svg.addEventListener('pointerout', e => { if (e.target.closest('.hike')) hover(null); tip.hidden = true; });
    svg.addEventListener('click', e => {
        tip.hidden = true;
        if (picking) { const r = svg.getBoundingClientRect(), x = vb.x + (e.clientX - r.left) / r.width * vb.w, y = vb.y + (e.clientY - r.top) / r.height * vb.h; picking = false; return update({ point: { x: Math.round(x), y: Math.round(y) }, pin: null }); }
        const hike = e.target.closest('.hike'), park = e.target.closest('.park'), lift = e.target.closest('.top'), poi = e.target.closest('.poi');
        if (park || lift) { const id = park ? park.dataset.park : lift.closest('.lift').dataset.lift; return setPin(state.pin?.id === id ? null : { kind: park ? 'park' : 'lift', id }); }
        if (poi) { const k = 'via:' + poi.dataset.place; return setWish(k, state.wish[k] ? 0 : 2); }
        if (hike) return update({ selected: +hike.dataset.hike });
    });
    flagsEl.addEventListener('click', e => {
        const b = e.target.closest('[data-unflag]'); if (!b) return;
        b.dataset.unflag === 'pin' ? setPin(null) : setWish(b.dataset.unflag, 0);
    });
    // A resize keeps the scale and the centre; once it settles the map fits again.
    let lastW = 0;
    new ResizeObserver(() => {
        const W = el.clientWidth, H = el.clientHeight;
        if (!W || !H) return;
        if (fitted && lastW) { Object.assign(vb, box(vb.x + vb.w / 2, vb.y + vb.h / 2, vb.w / lastW)); showBox(); }
        lastW = W;
        clearTimeout(settle); settle = setTimeout(() => { fitKey = null; renderMap(find()); }, fitted ? 220 : 0);
    }).observe(el);
}
const toScreen = (x, y) => [(x - vb.x) / vb.w * mapEl.clientWidth, (y - vb.y) / vb.h * mapEl.clientHeight];
const reachRadius = () => (state.pin || !state.car ? WALK[state.walk] : Math.max(WALK[state.walk], state.road / ROAD)) * PX_KM;
const setShow = (g, how) => g.setAttribute('data-show', how);
function renderMap(found) {
    if (!svg) return;
    const q = s => svg.querySelector(s), shown = new Set(found.map(r => r.h.id)), hot = state.hover ?? state.selected, hotRow = found.find(r => r.h.id === hot);
    for (const g of svg.querySelectorAll('.hike')) { const id = +g.dataset.hike; g.classList.toggle('off', !shown.has(id)); g.classList.toggle('hot', id === hot); }
    mapEl.classList.toggle('focus', !!hotRow);
    // Trailheads only for the ways to travel that are on: the access of each result names them.
    const parks = new Set(), lifts = new Set(), W = WALK[state.walk];
    for (const r of found) for (const a of r.acc) {
        if (a.park) parks.add(a.park.id);
        if (a.lift) lifts.add(a.lift.id);
        if (a.kind === 'any') { const n = parkAt(r.h); if (n) parks.add(n.park.id); for (const l of r.h.lifts) if (l.walk <= W) lifts.add(l.lift.id); }
        if (r.h.lr) lifts.add(r.h.lr);
    }
    const hotAcc = hotRow?.acc[0], hotTrail = hotAcc && (hotAcc.park?.id || hotAcc.lift?.id || (hotAcc.kind === 'any' && (parkAt(hotRow.h)?.park.id || liftAt(hotRow.h)?.lift.id)));
    const level = (id, used) => state.pin?.id === id ? 'pin' : hotTrail === id ? 'hot' : used ? 'quiet' : 'hide';
    for (const g of svg.querySelectorAll('.park')) setShow(g, level(g.dataset.park, parks.has(g.dataset.park)));
    for (const g of svg.querySelectorAll('.lift')) { const how = level(g.dataset.lift, lifts.has(g.dataset.lift)); setShow(g, how); setShow(g.querySelector('.top'), how); }
    const kinds = new Set(Object.keys(state.wish).map(k => WISH[k]?.place).filter(Boolean)), passed = new Set(found.flatMap(r => [...r.h.via]));
    for (const g of svg.querySelectorAll('.poi')) {
        const id = g.dataset.place;
        setShow(g, state.wish['via:' + id] ? 'pin' : hotRow?.h.via.has(id) ? 'hot' : kinds.has(PLACE[id].kind) && passed.has(id) ? 'quiet' : 'hide');
    }
    const o = state.point, at = pinAt() || o;
    q('.you').toggleAttribute('hidden', !o || !!state.pin);
    if (o) { q('.you').setAttribute('transform', `translate(${o.x} ${o.y})`); q('.you text').textContent = o.gps ? 'You' : 'Start'; }
    // The way from the start to the hovered or selected hike, as a straight hint of the road or the walk.
    let line = '';
    if (hotAcc && o && !state.pin) {
        const end = hotAcc.kind === 'foot' ? nearest(hotRow.h, o) : hotAcc.park || hotAcc.lift?.valley;
        if (end) line = `M${o.x} ${o.y}L${end.x.toFixed(1)} ${end.y.toFixed(1)}`;
        q('.approach').setAttribute('class', 'approach ' + (hotAcc.kind === 'car' || hotAcc.by === 'car' ? 'drive' : 'foot'));
    }
    q('.approach').setAttribute('d', line || 'M0 0');
    q('.approach').toggleAttribute('hidden', !line);
    const r = reachRadius();
    q('.reach').toggleAttribute('hidden', !at); q('.dim').toggleAttribute('hidden', !at);
    if (at) for (const el of [q('.reach'), q('.hole')]) { el.style.r = r + 'px'; el.style.cx = at.x + 'px'; el.style.cy = at.y + 'px'; }
    mapEl.classList.toggle('picking', picking); mapEl.querySelector('.pick-banner').hidden = !picking;
    renderFlags();
    fitMap(found, at, r);
}
function renderFlags() {
    const flags = [];
    const p = pinned();
    if (p) { const at = pinAt(); flags.push({ x: at.x, y: at.y, key: 'pin', html: `${state.pin.kind === 'park' ? pbadge : icon('lift', 14)}${state.pin.kind === 'park' ? p.name : p.short + ' top'}`, label: 'Unpin ' + pinName() }); }
    for (const k of Object.keys(state.wish).filter(k => k.startsWith('via:'))) { const pl = PLACE[k.slice(4)]; flags.push({ x: pl.x, y: pl.y, key: k, html: `${icon(pl.kind, 14)}Passes ${pl.name}`, label: 'Remove: passes ' + pl.name }); }
    const key = flags.map(f => f.key).join('|');
    if (key !== flagsKey) {
        flagsKey = key;
        flagsEl.innerHTML = flags.map(f => `<div class="flag" data-x="${f.x}" data-y="${f.y}"><span>${f.html}</span><button type="button" data-unflag="${f.key}" aria-label="${f.label}">${icon('close', 14)}</button></div>`).join('');
    }
    placeFlags();
}
function placeFlags() {
    // A flag near an edge slides inwards so that its remove button stays on the map.
    for (const f of flagsEl.children) { const [x, y] = toScreen(+f.dataset.x, +f.dataset.y), half = f.offsetWidth / 2 + 8; f.style.left = Math.max(half, Math.min(mapEl.clientWidth - half, x)) + 'px'; f.style.top = y + 'px'; }
}
function showBox() {
    svg.setAttribute('viewBox', `${vb.x.toFixed(1)} ${vb.y.toFixed(1)} ${vb.w.toFixed(1)} ${vb.h.toFixed(1)}`);
    const k = 1.15 * vb.w / Math.max(1, mapEl.clientWidth);
    for (const el of svg.querySelectorAll('.sz')) el.setAttribute('transform', `scale(${k.toFixed(3)})`);
    for (const el of svg.querySelectorAll('.valley')) el.setAttribute('r', (3.5 * k).toFixed(2));
    mapEl.classList.toggle('far', vb.w > FAR);
    placeFlags();
}
// A view of scale s (map units per pixel) around a centre, kept on the picture of the map.
function box(cx, cy, s) {
    const W = mapEl.clientWidth || 800, H = mapEl.clientHeight || 600;
    s = Math.min(s, MAP_W / W, MAP_H / H);
    const w = W * s, h = H * s, clamp = (v, hi) => Math.max(0, Math.min(hi, v));
    return { w, h, x: clamp(cx - w / 2, MAP_W - w), y: clamp(cy - h / 2, MAP_H - h) };
}
// The map zooms to the selected route, or else to what the rider can reach.
function fitMap(found, at, r) {
    const sel = found.find(x => x.h.id === state.selected);
    const key = JSON.stringify([sel ? sel.h.id : null, state.point, state.walk, state.car, state.road, state.large, state.lift, state.pin]);
    if (key === fitKey) return;
    fitKey = key;
    let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
    const add = (x, y, pad = 0) => { x0 = Math.min(x0, x - pad); x1 = Math.max(x1, x + pad); y0 = Math.min(y0, y - pad); y1 = Math.max(y1, y + pad); };
    if (sel) {
        for (const [x, y] of sel.h.samples) add(x, y);
        const a = sel.acc[0], end = a.park || (a.lift && a.lift.valley) || (a.kind === 'foot' && state.point);
        if (end) add(end.x, end.y);
    } else {
        if (at) add(at.x, at.y, r);
        for (const h of HIKES) if ((at ? access(h).length : true)) for (const [x, y] of h.samples) add(x, y);
    }
    const W = mapEl.clientWidth || 800, H = mapEl.clientHeight || 600, m = 36, aw = Math.max(80, W - 2 * m), ah = Math.max(80, H - 2 * m);
    const to = box((x0 + x1) / 2, (y0 + y1) / 2, Math.max((x1 - x0) / aw, (y1 - y0) / ah, 220 / aw));
    const from = { ...vb }, start = performance.now(), run = ++fitAnim, first = !fitted;
    fitted = true;
    const step = now => {
        if (run !== fitAnim) return;
        const t = reduced || first ? 1 : Math.min(1, (now - start) / 520), e = 1 - Math.pow(1 - t, 4);
        for (const k of ['x', 'y', 'w', 'h']) vb[k] = from[k] + (to[k] - from[k]) * e;
        showBox();
        if (t < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
}

return {
    get state() { return state; }, update, render, reset, isFresh, set page(p) { page = { ...page, ...p }; },
    icon, fmt, shapeTag: h => SHAPES[h.shape].tag, accessSteps, wishLabel, wishIcon, niceWishes, queryHtml, countText, mountFilters, mountGuide, mountMap, detailHtml, hover, emptyHtml,
};
})();
