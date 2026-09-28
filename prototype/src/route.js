/* The built route of a Black Forest plan: points with a type and legs with a mode between them.
   Routing is fake: a routed leg is the kit path when one exists, else a smooth curve; a straight
   leg is a line; a drawn leg is the rider's stroke. Figures are a plain estimate. */

const Route = (() => {
  const M = DATA.MAPS.bf;
  const near = (pos) => DATA.PLACES.filter((p) => p.map === 'bf').map((p) => ({ p, d: Math.hypot((p.x - pos.x) * M.kmx, (p.y - pos.y) * M.kmy) })).sort((a, b) => a.d - b.d)[0];
  const elevOf = (pos) => near(pos).p.elev;
  let seq = 0;
  const pt = (pos, kind, name, extra) => Object.assign({ id: `p${++seq}-${Date.now() % 1000}`, name, kind, map: 'bf', x: pos.x, y: pos.y, elev: elevOf(pos) }, extra);

  function fresh(plan) {
    if (plan.kind === 'import') {
      const s = Line.sample('route-bf-import', 200);
      return { pts: [pt({ x: s[0][0], y: s[0][1] }, 'start', 'Start', { fixed: true }), pt({ x: s[200][0], y: s[200][1] }, 'pass', 'End', { fixed: true })], legs: [{ mode: 'routed', path: 'route-bf-import', n: 200, km: 312, climb: 6480, time: 1560, prof: 'prof-bf-import', fixed: true }], route: null, visits: [], bike: null, goal: null };
    }
    const here = DATA.place(plan.here);
    return { pts: [pt(here, 'start', here.name, { place: here.id })], legs: [], route: null, visits: [], bike: null, goal: null };
  }
  // "Use this route": the destination becomes the second point, the option's path the leg.
  function useOption(mut, data, opt, id) {
    const to = DATA.place(data.to), from = DATA.place(data.from);
    Object.assign(mut.pts[0], { x: from.x, y: from.y, name: from.name, place: from.id, elev: from.elev });
    mut.pts.length = 1; mut.pts.push(pt(to, 'pass', to.name, { place: to.id }));
    mut.legs = [{ mode: 'routed', path: opt.path, n: 120, km: opt.km, climb: opt.climb, time: opt.time, prof: opt.prof }];
    mut.route = { id, opt: opt.id };
  }
  function legPts(mut, i) {
    const L = mut.legs[i], a = mut.pts[i], b = mut.pts[i + 1];
    if (L.path) return Line.sample(L.path, L.n);
    if (L.mode === 'straight') return [[a.x, a.y], [b.x, b.y]];
    if (L.mode === 'drawn' && L.d) return [[a.x, a.y], ...L.d, [b.x, b.y]];
    return Line.curve(a, b);
  }
  // Estimated figures of a leg: km from its length, climb from the points' heights.
  function legFigures(mut, i) {
    const L = mut.legs[i]; if (L.path) return { km: L.km, climb: L.climb, time: L.time };
    const a = mut.pts[i], b = mut.pts[i + 1], len = Line.lengthKm('bf', legPts(mut, i)), km = Math.round(len * (L.mode === 'straight' ? 1 : 1.12) * 10) / 10;
    const climb = Math.max(0, b.elev - a.elev) + Math.round(km * (L.mode === 'straight' ? 3 : 8));
    return { km, climb, time: Math.round((km / 16) * 60 + (climb / 600) * 60) };
  }
  function figures(mut) {
    const f = { km: 0, climb: 0, time: 0 };
    mut.legs.forEach((_, i) => { const g = legFigures(mut, i); f.km += g.km; f.climb += g.climb; f.time += g.time; });
    for (const v of mut.visits) { f.km += v.km || 0; f.climb += v.climb || 0; f.time += Math.round((v.km || 0) * 4); }
    return { km: Math.round(f.km * 10) / 10, climb: Math.round(f.climb / 10) * 10, time: Math.round(f.time / 5) * 5, days: 1 + mut.pts.filter((p) => p.kind === 'sleep').length };
  }
  // The whole line with km, for the profile window and the nearest km.
  function polyline(mut) {
    const pts = []; let km = 0;
    mut.legs.forEach((_, i) => { const P = legPts(mut, i), len = legFigures(mut, i).km, l = Line.lengthKm('bf', P) || 1; let acc = 0; P.forEach((q, j) => { if (j) acc += Math.hypot((q[0] - P[j - 1][0]) * M.kmx, (q[1] - P[j - 1][1]) * M.kmy); if (j || !i) pts.push([q[0], q[1], km + (acc / l) * len]); }); km += len; });
    return { map: 'bf', pts };
  }
  // Elevation samples of the route: the kit profile for a kit leg, a line with a bump otherwise.
  function profile(mut) {
    const out = []; let km = 0;
    mut.legs.forEach((L, i) => {
      const f = legFigures(mut, i), a = mut.pts[i], b = mut.pts[i + 1];
      if (L.prof) for (const [k, e] of Line.profile(L.prof + '-line')) out.push([km + k, e]);
      else for (let j = 0; j <= 8; j++) { const t = j / 8; out.push([km + f.km * t, a.elev + (b.elev - a.elev) * t + Math.sin(t * Math.PI) * Math.min(120, f.climb * 0.4)]); }
      km += f.km;
    });
    return out.length ? out : [[0, mut.pts[0].elev]];
  }
  const nameNear = (pos) => { const n = near(pos); return n.d <= 2.5 ? n.p.name : null; };
  function append(mut, pos) { const nm = nameNear(pos); mut.pts.push(pt(pos, 'pass', nm || `Point ${mut.pts.length + 1}`, nm ? { place: near(pos).p.id } : {})); mut.legs.push({ mode: 'routed' }); mut.route = null; }
  // Dragging a leg inserts a shape point in it; both halves keep the leg's mode.
  function insert(mut, i, pos) { const L = mut.legs[i]; mut.pts.splice(i + 1, 0, pt(pos, 'shape', `Point ${i + 2}`)); mut.legs.splice(i, 1, { mode: L.path ? 'routed' : L.mode }, { mode: L.path ? 'routed' : L.mode }); if (L.path) mut.route = null; }
  function move(mut, i, pos) { Object.assign(mut.pts[i], { x: pos.x, y: pos.y, elev: elevOf(pos) }); }
  // Removing a point joins its two legs; nothing else changes.
  function remove(mut, i) {
    if (i === 0 || mut.pts[i].fixed) return;
    mut.pts.splice(i, 1);
    if (i === mut.legs.length) mut.legs.pop(); else { const m = mut.legs[i - 1].mode; mut.legs.splice(i - 1, 2, { mode: m === 'drawn' ? 'routed' : m }); }
    if (mut.legs.some((L) => !L.path)) mut.route = mut.route && mut.legs.every((L) => L.path) ? mut.route : null;
  }
  function setMode(mut, i, mode) { const L = mut.legs[i]; if (L.fixed) return; mut.legs[i] = { mode, d: mode === 'drawn' ? L.d : undefined }; if (L.path) mut.route = null; }
  function setDrawn(mut, i, stroke) { mut.legs[i] = { mode: 'drawn', d: stroke }; }
  return { fresh, useOption, legPts, legFigures, figures, polyline, profile, append, insert, move, remove, setMode, setDrawn, nameNear, near };
})();
