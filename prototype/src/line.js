/* Line geometry: kit paths sampled into polylines with a km at every point, so a day end, a
   scrubber or a map window can be turned into a place on the line and back. A polyline is
   {map, pts: [[x, y, km], ...]} sorted by km. Not a routing engine: a fake "routed" leg is a
   smooth curve, a straight leg a line, a drawn leg the rider's stroke. */

const Line = (() => {
  const samples = {};
  // n points along a kit path, in its own map units.
  function sample(id, n) {
    const key = id + ':' + n;
    if (samples[key]) return samples[key];
    const p = document.getElementById(id), L = p.getTotalLength(), out = [];
    for (let i = 0; i <= n; i++) { const q = p.getPointAtLength((L * i) / n); out.push([q.x, q.y]); }
    return (samples[key] = out);
  }
  // Points of a kit path with km spread linearly from k0 to k1.
  const withKm = (pts, k0, k1) => pts.map(([x, y], i) => [x, y, k0 + ((k1 - k0) * i) / (pts.length - 1)]);
  const join = (map, parts) => ({ map, pts: parts.flat() });

  function pointAt(line, km) {
    const P = line.pts; if (km <= P[0][2]) return { map: line.map, x: P[0][0], y: P[0][1] };
    let lo = 0, hi = P.length - 1;
    while (hi - lo > 1) { const m = (lo + hi) >> 1; if (P[m][2] <= km) lo = m; else hi = m; }
    const a = P[lo], b = P[hi], u = b[2] > a[2] ? (km - a[2]) / (b[2] - a[2]) : 0;
    return { map: line.map, x: a[0] + (b[0] - a[0]) * Math.min(1, u), y: a[1] + (b[1] - a[1]) * Math.min(1, u) };
  }
  // The km range of the line inside a rect {x, y, w, h} in the line's map, or null.
  function kmRange(line, r) {
    let a = Infinity, b = -Infinity;
    for (const [x, y, km] of line.pts) if (x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h) { a = Math.min(a, km); b = Math.max(b, km); }
    return a <= b ? [a, b] : null;
  }
  // The nearest point of the line to a position in the line's map: {km, dist} (dist in map units).
  function nearest(line, pos) {
    let best = { km: 0, dist: Infinity };
    const P = line.pts;
    for (let i = 1; i < P.length; i++) {
      const [ax, ay, ak] = P[i - 1], [bx, by, bk] = P[i], dx = bx - ax, dy = by - ay, l2 = dx * dx + dy * dy;
      const t = l2 ? Math.max(0, Math.min(1, ((pos.x - ax) * dx + (pos.y - ay) * dy) / l2)) : 0;
      const d = Math.hypot(pos.x - (ax + dx * t), pos.y - (ay + dy * t));
      if (d < best.dist) best = { km: ak + (bk - ak) * t, dist: d };
    }
    return best;
  }
  function slice(line, a, b) {
    const out = [pointAt(line, a)].map((p) => [p.x, p.y]);
    for (const [x, y, km] of line.pts) if (km > a && km < b) out.push([x, y]);
    const e = pointAt(line, b); out.push([e.x, e.y]);
    return out;
  }
  const pathD = (pts) => (pts.length ? 'M' + pts.map(([x, y]) => `${x.toFixed(1)} ${y.toFixed(1)}`).join('L') : '');
  const lengthKm = (map, pts) => { const m = DATA.MAPS[map]; let l = 0; for (let i = 1; i < pts.length; i++) l += Math.hypot((pts[i][0] - pts[i - 1][0]) * m.kmx, (pts[i][1] - pts[i - 1][1]) * m.kmy); return l; };
  // A fake routed leg: a cubic bend between two points, its side fixed by the endpoints.
  function curve(a, b) {
    const dx = b.x - a.x, dy = b.y - a.y, d = Math.hypot(dx, dy) || 1, s = ((Math.round(a.x + a.y + b.x + b.y) % 7) - 3) / 3;
    const nx = (-dy / d) * d * 0.22 * s, ny = (dx / d) * d * 0.22 * s;
    const c1 = { x: a.x + dx * 0.3 + nx, y: a.y + dy * 0.3 + ny }, c2 = { x: a.x + dx * 0.7 - nx * 0.6, y: a.y + dy * 0.7 - ny * 0.6 }, out = [];
    for (let i = 0; i <= 24; i++) { const t = i / 24, u = 1 - t; out.push([u * u * u * a.x + 3 * u * u * t * c1.x + 3 * u * t * t * c2.x + t * t * t * b.x, u * u * u * a.y + 3 * u * u * t * c1.y + 3 * u * t * t * c2.y + t * t * t * b.y]); }
    return out;
  }
  // The elevation samples of a kit profile path: [[km, m], ...].
  const profiles = {};
  function profile(id) {
    if (profiles[id]) return profiles[id];
    const n = document.getElementById(id).getAttribute('d').match(/-?\d+\.?\d*/g).map(Number), out = [];
    for (let i = 0; i + 1 < n.length; i += 2) out.push([n[i], (300 - n[i + 1]) * 10]);
    return (profiles[id] = out);
  }
  function elevAt(samples, km) {
    let lo = 0, hi = samples.length - 1;
    if (km <= samples[0][0]) return samples[0][1]; if (km >= samples[hi][0]) return samples[hi][1];
    while (hi - lo > 1) { const m = (lo + hi) >> 1; if (samples[m][0] <= km) lo = m; else hi = m; }
    const a = samples[lo], b = samples[hi]; return a[1] + ((b[1] - a[1]) * (km - a[0])) / (b[0] - a[0] || 1);
  }
  // Cumulative climb up to each sample, for a climb between two kms.
  function climbTable(samples) { let c = 0; return samples.map(([km, e], i) => { if (i && e > samples[i - 1][1]) c += e - samples[i - 1][1]; return [km, c]; }); }
  const climbBetween = (table, a, b) => elevAt(table, b) - elevAt(table, a);

  return { sample, withKm, join, pointAt, kmRange, nearest, slice, pathD, lengthKm, curve, profile, elevAt, climbTable, climbBetween };
})();
