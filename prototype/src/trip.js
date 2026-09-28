/* The Alps trip: the days derive from a list of day ends (trip km). A rest day is a zero-length
   day. A day's figures come from the content sheet's base days, weighted by the share of each
   base day it covers (km share for distance, profile climb share for climb, a mix for time), so
   the sheet's figures hold at rest and move smoothly with a day end. The line never changes. */

const Trip = (() => {
  const TOTAL = DATA.TRIP.km, MIN_DAY = 5;
  const BASE = []; { let s = 0; for (const d of DATA.DAYS) { BASE.push({ ...d, start: s, end: s + d.km }); s += d.km; } }
  let lines = null, climb = null;

  function init() {
    const riding = BASE.filter((d) => !d.rest), b4 = BASE[3];
    const alps = Line.join('alps', riding.map((d) => Line.withKm(Line.sample(`route-alps-d${d.n}`, 140), d.start, d.end)));
    // The Day 4 map also carries the other days, converted from the Alps map: zooming never hides a day.
    const inD4 = (pts) => pts.map(([x, y, km]) => { const c = MapView.convert({ map: 'alps', x, y }, 'd4'); return [c.x, c.y, km]; });
    const d4 = Line.join('d4', [inD4(alps.pts.filter((p) => p[2] < b4.start)), Line.withKm(Line.sample('route-d4', 500), b4.start, b4.end), inD4(alps.pts.filter((p) => p[2] > b4.end))]);
    lines = { alps, d4 };
    climb = Line.climbTable(Line.profile('prof-alps-line'));
    for (const p of DATA.PLACES) if (p.x == null && p.day) Object.assign(p, pointAt(tripKm(p), 'alps'));
  }
  const line = (map) => lines[map === 'd4' ? 'd4' : 'alps'];
  const pointAt = (km, map) => Line.pointAt(line(map), km);
  const tripKm = (p) => BASE[p.day - 1].start + p.km;
  const fresh = () => ({ ends: BASE.slice(0, -1).map((d) => d.end), names: {}, points: [], modes: {}, stops: {}, bike: null, goal: null });

  // Figures of the stretch [a, b]: each base day contributes its share.
  function figures(a, b) {
    let km = 0, cl = 0, t = 0;
    for (const d of BASE) {
      if (d.rest) continue;
      const s = Math.max(a, d.start), e = Math.min(b, d.end); if (e <= s) continue;
      const fk = (e - s) / d.km, tot = Line.climbBetween(climb, d.start, d.end) || 1, fc = Line.climbBetween(climb, s, e) / tot;
      km += e - s; cl += d.climb * fc; t += d.time * (0.53 * fk + 0.47 * fc);
    }
    return { km: Math.round(km), climb: Math.round(cl / 10) * 10, time: Math.round(t / 5) * 5 };
  }
  const dateOf = (i) => { const d = new Date(DATA.TRIP.start + 'T12:00:00'); d.setDate(d.getDate() + i); const w = d.toLocaleDateString('en-GB', { weekday: 'short' }); return { date: `${w} ${d.getDate()} ${d.toLocaleDateString('en-GB', { month: 'short' })}`, wd: w.toLowerCase() }; };
  // The name of a point on the line: the nearest named place within 4 km, else its km.
  function endName(km) {
    let best = null;
    for (const p of DATA.PLACES) if (p.day && p.km != null && !p.off && !DATA.KINDS[p.kind]) { const d = Math.abs(tripKm(p) - km); if (d <= 4 && (!best || d < best.d)) best = { p, d }; }
    return best ? best.p.name : `km ${Math.round(km)}`;
  }
  // The days the rider sees, with their figures and colours.
  function days(mut) {
    const ends = mut.ends.concat(TOTAL), out = []; let s = 0, ci = 0;
    ends.forEach((e, i) => {
      const rest = e === s, f = rest ? { km: 0, climb: 0, time: 0 } : figures(s, e), stop = mut.stops[i];
      if (stop) { f.km = Math.round(f.km + stop.km); f.climb += stop.climb; f.time += Math.round(stop.km * 4); }
      const from = endName(s), to = stop ? DATA.place(stop.place).name.split(',')[0] : endName(e);
      out.push({ i, n: i + 1, start: s, end: e, rest, ...f, from, to, name: mut.names[i] || null, title: mut.names[i] || (rest ? `Rest day · ${to}` : `${from} → ${to}`), ...dateOf(i), ci: rest ? null : ci++ % 4 });
      s = e;
    });
    return out;
  }
  const dayAt = (D, km) => D.find((d) => !d.rest && km >= d.start && km <= d.end) || D[D.length - 1];
  // Set the end of day i (a following rest day moves with it).
  function setEnd(mut, i, km) {
    const lo = (i ? mut.ends[i - 1] : 0) + MIN_DAY, hiIdx = mut.ends[i + 1] === mut.ends[i] ? i + 2 : i + 1, hi = (mut.ends[hiIdx] == null ? TOTAL : mut.ends[hiIdx]) - MIN_DAY;
    km = Math.max(lo, Math.min(hi, km)); if (!Number.isFinite(km)) return;
    mut.ends[i] = km; if (hiIdx === i + 2) mut.ends[i + 1] = km; delete mut.stops[i];
  }
  const splitDay = (mut, i) => { const D = days(mut), d = D[i]; if (d.rest) return; mut.ends.splice(i, 0, Math.round((d.start + d.end) / 2)); shiftKeys(mut, i, 1); };
  const joinDay = (mut, i) => { if (i >= mut.ends.length) return; mut.ends.splice(i, 1); shiftKeys(mut, i, -1); };
  function shiftKeys(mut, from, by) { for (const k of ['names', 'stops']) { const o = {}; for (const [j, v] of Object.entries(mut[k])) o[+j >= from ? +j + by : +j] = v; mut[k] = o; } }
  // Ends the day at a place: on the line at its km, or off the line by an out-and-back spur or a detour through it.
  function endAtPlace(mut, i, p, via) {
    setEnd(mut, i, tripKm(p));
    if (via && p.off) mut.stops[i] = { place: p.id, via, km: via === 'outback' ? Math.round(p.off * 2 * 10) / 10 : Math.round(p.off * 1.2 * 10) / 10, climb: via === 'outback' ? p.climb || 0 : Math.round((p.climb || 0) / 2) };
  }

  // The points of a day: its start, the passes and stops on it, its end; each with its trip km.
  function points(mut, day) {
    const pts = [{ id: `end${day.i - 1}`, name: day.from, kind: 'sleep', km: day.start, fixed: true }];
    for (const p of DATA.PLACES) if (p.kind === 'pass' && p.day && p.km != null) { const k = tripKm(p); if (k > day.start && k < day.end) pts.push({ id: p.id, name: p.name, kind: 'pass', km: k, place: p.id, pass: true }); }
    for (const p of mut.points) if (p.km > day.start && p.km < day.end) pts.push(p);
    pts.push({ id: `end${day.i}`, name: day.to, kind: 'sleep', km: day.end, fixed: true, stop: mut.stops[day.i] });
    return pts.sort((a, b) => a.km - b.km);
  }
  const modeKey = (a, b) => `${a.id}|${b.id}`;
  const mode = (mut, a, b) => mut.modes[modeKey(a, b)] || 'routed';
  function addPoint(mut, pt) { mut.points.push(pt); }
  function removePoint(mut, id) { mut.points = mut.points.filter((p) => p.id !== id); }
  // Changing a type to Sleep splits the day at the point; a day end is a point, so it stays a Sleep.
  function setKind(mut, id, kind) {
    const p = mut.points.find((x) => x.id === id); if (!p) return;
    if (kind === 'sleep') { const i = mut.ends.findIndex((e) => e > p.km); mut.ends.splice(i < 0 ? mut.ends.length : i, 0, p.km); shiftKeys(mut, i < 0 ? mut.ends.length - 1 : i, 1); removePoint(mut, id); }
    else p.kind = kind;
  }
  // Legs of a day as [kmA, kmB, mode, stroke] for the map.
  function legs(mut, day) { const P = points(mut, day), out = []; for (let i = 1; i < P.length; i++) out.push({ a: P[i - 1], b: P[i], mode: mode(mut, P[i - 1], P[i]), key: modeKey(P[i - 1], P[i]) }); return out; }
  // The reading under the scrubber: day, km, height, and the next pass.
  function reading(mut, km) {
    const D = days(mut), d = dayAt(D, km), e = Math.round(Line.elevAt(Line.profile('prof-alps-line'), km));
    let next = null;
    for (const p of DATA.PLACES) if (p.kind === 'pass' && p.day && p.km != null) { const k = tripKm(p) - km; if (k >= 0 && k <= d.end - km && (!next || k < next.k)) next = { p, k }; }
    const tail = next ? (next.k < 0.5 ? next.p.name : `${next.p.name} in ${Math.round(next.k)} km`) : d.to;
    return { b: `Day ${d.n} · km ${Math.round(km - d.start)} · ${e.toLocaleString('en-GB')} m`, span: tail, day: d };
  }
  return { init, BASE, TOTAL, MIN_DAY, line, pointAt, tripKm, fresh, figures, days, dayAt, endName, setEnd, splitDay, joinDay, endAtPlace, points, legs, mode, modeKey, addPoint, removePoint, setKind, reading };
})();
