/* The scene: the map layers (the line per day and per leg mode, the points, the day ends, the
   pins, the answer's marks) and the profile's home and window. The profile shows the stretch of
   the line inside the visible map; the map drives it. */

const Scene = (() => {
  const { esc, icon, num, hm, km1 } = Answers;
  const $ = (id) => document.getElementById(id);
  const mk = (pos, o) => {
    const c = MapView.convert(pos, MapView.view.map), r = o.r || 12, ir = o.ir || 7, act = o.act ? `data-act="${o.act}" data-id="${esc(o.id || '')}" data-n="${o.n || ''}"` : '';
    return `<g class="mk ${o.cls || ''}" style="transform:translate(${c.x.toFixed(1)}px,${c.y.toFixed(1)}px) scale(var(--mk));${o.style || ''}" ${act}><circle r="${Math.max(22, r + 8)}" class="hit"/><circle r="${r}" class="ring"/>${o.icon ? `<use href="#${o.icon}" x="${-ir}" y="${-ir}" width="${ir * 2}" height="${ir * 2}"/>` : ''}${o.text ? `<text class="num" y="4.5">${o.text}</text>` : ''}${o.label ? `<text class="ml" x="${r + 6}" y="5">${esc(o.label)}</text>` : ''}</g>`;
  };
  const txt = (pos, label, cls = '', fill = '') => { const c = MapView.convert(pos, MapView.view.map); return `<g class="mk" style="transform:translate(${c.x}px,${c.y}px) scale(var(--mk))"><text class="ml ${cls}" text-anchor="middle" ${fill ? `style="fill:${fill}"` : ''}>${esc(label)}</text></g>`; };
  const use = (path, cls, act) => `<use href="#${path}" class="${cls}"/>` + (act ? `<use href="#${path}" class="rt-hit" ${act}/>` : '');
  // A leg: casing, the line in its mode, and a hit path.
  const leg = (pts, color, mode, hit) => { const d = Line.pathD(pts); return `<path d="${d}" class="rt-casing ${mode}"/><path d="${d}" class="rt-line ${mode}" style="stroke:${color}"/>${hit ? `<path d="${d}" class="rt-hit" ${hit}/>` : ''}`; };
  const cur = () => MapView.view.map;
  const near = (a, b, px) => { const A = MapView.toScreen(a), B = MapView.toScreen(b); return Math.hypot(A.x - B.x, A.y - B.y) < px; };
  let wasZoomed = null;                                              // pins show under 90 km of map width

  function renderMap(S) {
    const plan = S.plan(), mut = S.mut(), map = cur(), a = S.show() ? S.ans : null, zoomed = (wasZoomed = MapView.widthKm() < 90);
    const amarks = a ? Answers.marks(a, S) : [], hot = (amarks.find((m) => m.sel) || {}).pos || (S.callout && S.callout.pos) || null;
    let routes = '', marks = '';
    if (plan.kind === 'trip') {
      const D = Trip.days(mut), line = Trip.line(map), inLine = (k) => k >= line.pts[0][2] && k <= line.pts[line.pts.length - 1][2];
      for (const d of D) {
        if (d.rest) continue;
        const color = View.dayTok(d.ci);
        for (const L of Trip.legs(mut, d)) {
          const stroke = mut.strokes && mut.strokes[L.key];
          const pts = L.mode === 'straight' ? [Trip.pointAt(L.a.km, map), Trip.pointAt(L.b.km, map)].map((p) => [p.x, p.y]) : L.mode === 'drawn' && stroke ? stroke.pts.map((q) => { const c = MapView.convert({ map: stroke.map, x: q[0], y: q[1] }, map); return [c.x, c.y]; }) : Line.slice(line, L.a.km, L.b.km);
          routes += leg(pts, color, L.mode, `data-act="legTap" data-key="${esc(L.key)}" data-day="${d.i}"`);
        }
        const stop = mut.stops[d.i], sp = stop && DATA.place(stop.place);
        if (sp && inLine(d.end)) { const e = Trip.pointAt(d.end, map), q = MapView.convert(sp, map); routes += stop.via === 'through' ? leg([Trip.pointAt(d.end - 1.5, map), q, Trip.pointAt(d.end + 1.5, map)].map((p) => [p.x, p.y]), color, 'routed') : `<path d="${Line.pathD([[e.x, e.y], [q.x, q.y]])}" class="rt-spur"/>`; }
      }
      for (const p of mut.points) if (p.kind === 'visit' && p.place && inLine(p.km)) { const pl = DATA.place(p.place), e = Trip.pointAt(p.km, map), q = MapView.convert(pl, map); if (pl.off) routes += `<path d="${Line.pathD([[e.x, e.y], [q.x, q.y]])}" class="rt-spur"/>`; }
      if (inLine(0)) marks += mk(Trip.pointAt(0, map), { cls: 'start', r: 9 });
      for (const d of D) {
        if (d.rest || !inLine(d.end)) continue;
        const pos = mut.stops[d.i] ? DATA.place(mut.stops[d.i].place) : Trip.pointAt(d.end, map), last = d.i === D.length - 1;
        if (last) { marks += mk(pos, { cls: 'dest', r: 13, icon: 'i-flag', ir: 8 }); continue; }
        const label = zoomed && !(hot && near(pos, hot, 60)) ? `End of Day ${d.n} · ${d.to}` : null, pointed = S.pointed && S.pointed.day === d.n;
        marks += mk(pos, { cls: 'dend' + (pointed ? ' pointed' : ''), r: 13, text: d.n, act: 'pointEnd', n: d.n, label });
      }
      for (const d of D) if (!d.rest) for (const p of Trip.points(mut, d)) {
        if (p.fixed || !inLine(p.km) || (p.pass && !zoomed)) continue;
        const pos = p.place && DATA.place(p.place).off ? DATA.place(p.place) : Trip.pointAt(p.km, map);
        marks += p.pass ? mk(pos, { cls: 'pt pass', r: 6, act: 'pointTap', id: p.id }) : mk(pos, { cls: 'pt', r: p.kind === 'shape' ? 6 : 11, icon: p.kind === 'shape' ? null : View.kindIcon(p.kind), act: 'pointTap', id: p.id, label: p.kind === 'shape' ? null : p.name.split(',')[0] });
      }
      if (zoomed) for (const p of DATA.PLACES) if (p.map !== 'bf' && DATA.KINDS[p.kind] && DATA.KINDS[p.kind].sleep && !amarks.some((m) => m.id === p.id) && !mut.points.some((x) => x.place === p.id)) marks += mk(p, { cls: 'pin', r: 9, ir: 5, icon: DATA.KINDS[p.kind].icon, act: 'placeTap', id: p.id });
    } else {
      if (a && a.kind === 'route' && !a.noData) {
        routes = a.options.filter((o) => o !== a.sel).map((o) => use(o.path, 'rt-alt-casing') + use(o.path, 'rt-alt', `data-act="selOpt" data-id="${o.id}"`)).join('') + use(a.sel.path, 'rt-casing') + use(a.sel.path, 'rt-line');
        marks += a.options.filter((o) => o.label).map((o) => txt({ map: 'bf', x: o.label[1], y: o.label[2] }, o.label[0], o === a.sel ? 'rt' : '')).join('');
      } else mut.legs.forEach((L, i) => { routes += leg(Route.legPts(mut, i), 'var(--route)', L.mode, `data-act="legTap" data-key="${i}" ${L.fixed ? '' : `data-leg="${i}"`}`); });
      for (const v of mut.visits) if (v.path) routes += use(v.path, 'rt-casing') + use(v.path, 'rt-line');
      if (a && a.kind === 'change' && a.type === 'visit') routes += use(a.ghost, 'rt-ghost');
      if (S.drawing) routes += `<path id="rt-draw" class="rt-draw"/>`;
      mut.pts.forEach((p, i) => {
        if (!i) { marks += mk(p, plan.here ? { cls: 'here', r: 9, act: 'pointTap', id: p.id } : { cls: 'start', r: 9, act: 'pointTap', id: p.id }); return; }
        marks += mk(p, { cls: 'pt' + (S.dragPt === i ? ' live' : ''), r: p.kind === 'shape' ? 6 : 11, icon: p.kind === 'shape' ? null : View.kindIcon(p.kind), act: 'pointTap', id: p.id, label: p.kind === 'shape' || (a && a.kind === 'route') ? null : p.name.split(',')[0] });
      });
      for (const v of mut.visits) marks += mk(DATA.place(v.place), { cls: 'pt', r: 11, icon: 'i-flag', label: v.name.split(',')[0] });
      if (zoomed) for (const p of DATA.PLACES) if (p.map === 'bf' && DATA.KINDS[p.kind] && DATA.KINDS[p.kind].sleep && !amarks.some((m) => m.id === p.id) && !mut.visits.some((v) => v.place === p.id)) marks += mk(p, { cls: 'pin', r: 9, ir: 5, icon: DATA.KINDS[p.kind].icon, act: 'placeTap', id: p.id });
    }
    for (const m of amarks) {
      if ((m.pos.map === 'bf') !== (map === 'bf')) continue;
      const cls = m.kind === 'result' ? (m.sel ? 'res sel' : 'res') + (m.dim ? ' dimmed' : '') : m.kind;
      marks += mk(m.pos, { cls, r: m.sel || m.kind === 'place' || m.kind === 'newEnd' ? 15 : 12, ir: m.sel ? 9 : 7, icon: m.icon || (m.kind === 'dest' ? 'i-flag' : m.kind === 'oldEnd' || m.kind === 'newEnd' ? 'i-tent' : null), act: m.id ? 'selRow' : null, id: m.id, label: m.label });
    }
    if (S.scrub != null && plan.kind === 'trip') marks += mk(Trip.pointAt(S.scrub, map), { cls: 'scrubpt', r: 7 });
    MapView.setLayers(routes, marks);
  }
  function drawPreview(stroke) { const p = $('rt-draw'); if (p) p.setAttribute('d', Line.pathD(stroke.map((q) => [q.x, q.y]))); }

  // ---- the profile: its window follows the map, its home follows the layout ----
  const lineOf = (S) => (S.plan().kind === 'trip' ? Trip.line(cur()) : Route.polyline(S.mut()));
  function window_(S) {
    const total = totalKm(S), line = lineOf(S); if (!line.pts.length) return [0, Math.max(1, total)];
    const r = Line.kmRange(line, MapView.viewRect(S.insets())); if (!r) return [0, total];
    if (r[1] - r[0] >= total * 0.96) return [0, total];
    const pad = (r[1] - r[0]) * 0.03; return [Math.max(0, r[0] - pad), Math.min(total, r[1] + pad)];
  }
  const totalKm = (S) => (S.plan().kind === 'trip' ? Trip.TOTAL : Route.figures(S.mut()).km);
  function spec(S) {
    const plan = S.plan(), mut = S.mut(), web = S.layout === 'web', a = S.show() ? S.ans : null, onPlot = web || S.land;
    if (plan.kind === 'trip') {
      const D = Trip.days(mut), ticks = a && a.kind === 'places' ? a.rows.filter((r) => Answers.onLine(r.p)).map((r) => ({ km: Trip.tripKm(r.p), id: r.p.id, sel: S.selRow === r.p.id })) : [];
      const gap = a && a.kind === 'gaps' ? a.items.filter((i) => i.band).map((i) => i.band) : [];
      return { key: 'trip', src: { use: 'prof-alps' }, max: 3000, win: S.win || [0, Trip.TOTAL], days: D.map((d) => ({ i: d.i, n: d.n, start: d.start, end: d.end, rest: d.rest, color: d.rest ? null : View.dayTok(d.ci), sel: S.selDay === d.n })),
        ends: D.filter((d) => !d.rest && d.i < D.length - 1).map((d) => ({ i: d.i, km: d.end, n: d.n, sel: S.selDay === d.n, color: View.dayTok(d.ci) })), ticks, marks: { ...DATA.BANDS, gap },
        scrub: S.scrub, reading: onPlot && S.scrub != null ? (() => { const r = Trip.reading(mut, S.scrub); return `${r.b} · ${r.span}`; })() : null, rows: onPlot, handlers: App.profileHandlers };
    }
    const o = mut.route && App.routeOption(mut.route), total = totalKm(S) || 1, surf = (plan.kind === 'import' ? { unknown: [[40, 81]] } : o && o.surface) || {};
    return { key: plan.id + ':' + mut.legs.map((L) => L.path || L.mode).join(','), src: { samples: Route.profile(mut) }, max: 1500, win: S.win || [0, total], days: [{ i: 0, n: 0, start: 0, end: total, color: 'var(--amber)', flat: true, sel: false }], ends: [], ticks: [], marks: surf, scrub: null, reading: null, rows: onPlot, handlers: App.profileHandlers };
  }
  // The profile's home: the website's bottom panel, the phone sheet's slot, or landscape's page.
  function renderProfile(S) {
    const web = S.layout === 'web', el = $('prof');
    if (web) {
      const plan = S.plan(), mut = S.mut(), coll = S.profCollapsed; let head;
      if (plan.kind === 'trip') { const D = Trip.days(mut), d = S.selDay && D[S.selDay - 1]; head = d && !d.rest ? `<span class="h">Day ${d.n}${S.dates ? ' · ' + d.date : ''}</span><span class="cap">${esc(d.title)} · ${d.km} km · ${num(d.climb)} m · ${hm(d.time)}</span><span class="right"><span class="cap">Whole trip · ${DATA.TRIP.facts}</span>` : `<span class="h">Whole trip</span><span class="cap">${DATA.TRIP.facts}</span><span class="right">`; }
      else { const f = Route.figures(mut); head = `<span class="h">${plan.kind === 'import' ? DATA.ROUTES.import.title : mut.route ? DATA.ROUTES[mut.route.id].title : mut.legs.length ? 'Route' : 'No route yet'}</span><span class="cap">${mut.legs.length ? `${km1(f.km)} km · ${num(f.climb)} m · ${hm(f.time)}` : ''}</span><span class="right">`; }
      el.classList.toggle('collapsed', coll);
      el.innerHTML = `<div class="ph">${head}<button class="btn ghost sm icon" data-act="collapseProf" title="${coll ? 'Expand' : 'Collapse'}">${icon(coll ? 'i-chev-r' : 'i-chev-d')}</button></span></div>${coll ? '' : `<div class="slot"></div>`}`;
      if (!coll && App.hasLine()) { el.querySelector('.slot').appendChild(App.pf); Profile.render(App.pf, spec(S)); }
      else if (!coll) el.querySelector('.slot').innerHTML = `<div class="note quiet">No profile yet.</div>`;
      return;
    }
    el.innerHTML = '';
    if (S.land) { if (App.hasLine()) Profile.render(App.pf, spec(S)); return; }
    const slot = $('pfslot');
    if (!App.hasLine()) { slot.innerHTML = `<div class="note quiet">No profile yet.</div>`; return; }
    if (App.pf.parentElement !== slot) { slot.innerHTML = ''; slot.appendChild(App.pf); }
    Profile.render(App.pf, spec(S));
  }
  // Called on every map move: the window follows the map, the callout follows its anchor, and the
  // marks are redrawn when a zoom crosses the pin threshold.
  function onMove(S) {
    if (App.hasLine()) { const w = window_(S); if (!S.win || Math.abs(w[0] - S.win[0]) > 0.2 || Math.abs(w[1] - S.win[1]) > 0.2) { S.win = w; if (App.pf.isConnected) Profile.render(App.pf, spec(S)); } }
    if ((MapView.widthKm() < 90) !== wasZoomed) renderMap(S);
    View.placeCallout(S);
  }
  return { renderMap, renderProfile, onMove, window: window_, drawPreview };
})();
