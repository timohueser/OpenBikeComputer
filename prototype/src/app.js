/* The app: one state object, undo/redo as snapshots of the plan's mutable part, an actions table
   reached by data-act, and one render pass. The plans page opens a plan in the planner. The
   website and the phone share the planner's DOM; CSS lays it out by S.layout. On the phone the
   sheet has two heights (the profile; plus the box and one list) and a full-screen search;
   landscape shows the profile alone. */

const App = (() => {
  const { esc, icon, num, hm, km1 } = Answers;
  const $ = (id) => document.getElementById(id);
  const S = {
    page: 'plans', layout: 'web', land: false, framed: false, rot: false, planId: 'alps', muts: {}, text: '', pointed: null, edits: {}, removed: {}, edited: false, focused: false, submitted: false,
    req: null, ans: null, routeTo: null, selDay: null, selRow: null, selOpt: 'shortest', picker: null, note: null, menu: null, menuAnchor: null, menuDay: null, menuKey: null, menuLeg: null, menuMode: null,
    dates: true, today: 3, pick: false, moreOpen: false, hist: [], future: [], profCollapsed: false, here: 'glottertal', frameKey: null,
    callout: null, expanded: new Set(), renaming: null, scrub: null, drawing: null, dragPt: null, dragEnd: null, win: null, prevDetent: 'collapsed',
    plan() { return DATA.PLANS.find((p) => p.id === S.planId); },
    mut() { return S.muts[S.planId] || (S.muts[S.planId] = S.plan().kind === 'trip' ? Trip.fresh() : Route.fresh(S.plan())); },
    preset() { const m = S.mut(), p = S.plan(); return { bike: m.bike || p.bike, goal: m.goal || p.goal }; },
    // The map frames into the area the nav bar and the sheet leave free; a search returns to the middle height.
    insets() { return phone() && !S.land ? { t: navH(), b: Sheet.full ? Sheet.heights.middle : Sheet.height } : { t: 0, b: 0 }; },
    show() { return !!(S.ans && (S.text || S.pointed || S.routeTo)); },
  };
  const phone = () => S.layout === 'phone';
  const navH = () => ($('nav').offsetHeight || 0);
  const app = () => $('app');
  let pf = null;
  const routeOption = (r) => DATA.ROUTES[r.id].options.find((o) => o.id === r.opt);
  const hasLine = () => S.plan().kind !== 'new' || S.mut().legs.length > 0;
  const trip = () => S.plan().kind === 'trip';

  // ---- the request → answer pipeline ----
  function run() {
    const plan = S.plan(), days = trip() ? Trip.days(S.mut()).filter((d) => !d.rest).map((d) => d.n) : [];
    const ctx = { plan, hasDates: S.dates && trip(), today: S.today, selDay: S.selDay, pointed: S.pointed, region: plan.map === 'bf' ? 'Black Forest' : 'Alps', map: plan.map, days };
    let req = Parser.parse(S.text, ctx);
    if (S.routeTo) req = { ...req, kind: 'route', to: S.routeTo, off: [], bike: S.preset().bike, goal: S.preset().goal };
    const E = S.edits;
    if (E.what && (req.kind === 'places' || (req.kind === 'none' && !S.text))) { req.kinds = E.what; req.kind = 'places'; }
    if (req.kind === 'places') { req.where = E.where || Parser.where(req, ctx); if (E.within != null) req.within = E.within; if ('open' in E) req.open = E.open; }
    if (req.kind === 'route') { if (E.bike) req.bike = E.bike; if (E.goal) req.goal = E.goal; }
    S.req = req;
    S.ans = (req.kind !== 'none' || S.submitted) && (S.text || S.pointed || S.routeTo) ? Answers.compute(req, S) : null;
    const a = S.ans, key = answerKey(a);
    if (key !== S.frameKey) S.selRow = null;
    if (a && a.kind === 'places') { const ids = a.rows.map((r) => r.p.id).concat(a.empty && a.empty.nearest ? [a.empty.nearest.p.id] : []); if (!ids.includes(S.selRow)) S.selRow = ids[0] || null; }
    if (a && a.kind === 'gaps') S.selRow = a.items.length ? (String(S.selRow).startsWith('gap') ? S.selRow : 'gap0') : null;
    if (a && a.kind === 'route' && a.from) S.here = a.from.id;
    autoFrame();
  }
  const answerKey = (a) => !a || a.noData ? null : a.kind === 'places' ? `places:${JSON.stringify(a.where)}` : a.kind === 'route' ? `route:${a.to.id}` : a.kind === 'change' ? `change:${a.type}` : a.kind === 'gaps' ? `gaps:${a.gapKind}` : `place:${a.place && a.place.id}`;
  // Frame the map once per new answer: the day of the where, the route, the stretch, the place.
  function autoFrame() {
    const a = S.ans, key = answerKey(a); if (!key) return;
    if (key === S.frameKey) return; S.frameKey = key;
    if (a.kind === 'places' && a.where.type === 'day') { S.selDay = a.where.day; frameDay(a.where.day); }
    else if (a.kind === 'places' && a.where.type === 'route') frameAll();
    else if (a.kind === 'places' && a.where.type === 'near') MapView.panTo(a.where.place, S.insets());
    else if (a.kind === 'route') frameBox(a.data.options.map((o) => MapView.bboxOfPath(o.path)).reduce(MapView.union), 'bf');
    else if (a.kind === 'change' && a.type === 'dayend') { S.selDay = a.day; frameDay(a.day); }
    else if (a.kind === 'change' && a.type === 'visit') frameBox(MapView.bboxOfPath('route-bf-import'), 'bf');
    else if (a.kind === 'gaps' && a.items.length) { if (a.gapKind === 'water' && trip()) { S.selDay = 4; frameDay(4); } else frameAll(); }
    else if ((a.kind === 'place' || a.kind === 'none') && a.place && a.place.map !== 'bf' === (S.plan().map !== 'bf')) MapView.panTo(a.place, S.insets());
  }
  const frameBox = (box, map, opts) => MapView.frame(map, box, S.insets(), Object.assign({ pad: 50 }, opts));
  function frameDay(n) {
    const d = Trip.days(S.mut())[n - 1]; if (!d || d.rest) return;
    const b4 = Trip.BASE[3], inD4 = d.start >= b4.start - 1 && d.end <= b4.end + 1, map = inD4 ? 'd4' : 'alps';
    frameBox(MapView.bboxOfPts(Line.slice(Trip.line(map), d.start, d.end)), map, inD4 ? {} : { max: 2.1, pad: 70 });
  }
  function frameAll(instant) {
    const plan = S.plan(), mut = S.mut();
    if (plan.kind === 'trip') frameBox(MapView.bboxOfPts(Trip.line('alps').pts), 'alps', { pad: 30, instant });
    else if (plan.kind === 'import') frameBox(MapView.bboxOfPath('route-bf-import'), 'bf', { instant });
    else if (mut.legs.length) frameBox(MapView.bboxOfPts(Route.polyline(mut).pts), 'bf', { instant, pad: 60 });
    else frameBox({ x: 150, y: 60, w: 760, h: 600 }, 'bf', { instant, pad: 10 });
  }
  const snapshot = () => JSON.stringify(S.mut());
  function commit(fn) { S.hist.push(snapshot()); S.future = []; fn(S.mut()); }
  function clearBox() { S.text = ''; S.pointed = null; S.edits = {}; S.removed = {}; S.edited = false; S.submitted = false; S.routeTo = null; S.picker = null; S.pick = false; S.moreOpen = false; S.ans = null; S.req = null; S.frameKey = null; }
  const selectedPlace = () => { const a = S.ans; if (!a) return null; const r = a.rows && a.rows.find((r) => r.p.id === S.selRow); return r ? r.p : a.empty && a.empty.nearest && a.empty.nearest.p.id === S.selRow ? a.empty.nearest.p : a.place || null; };
  const edit = (patch) => { Object.assign(S.edits, patch); S.edited = true; };
  const short = (n) => n.split(',')[0];
  const showOnMap = (pos) => { if (!MapView.inRect(pos, MapView.viewRect(S.insets()))) MapView.panTo(pos, S.insets()); };

  // A day end moves to a place: on the line at its km, or off the line the way the rider chose.
  function applyEnd(i, p, via) {
    commit((m) => Trip.endAtPlace(m, i, p, via));
    const D = Trip.days(S.mut());
    clearBox(); S.callout = null; S.note = { text: `Day ${D[i].n} now ends at ${short(p.name)}.`, undo: true }; S.selDay = D[i].n; frameDay(S.selDay);
  }
  function addStopPlace(p) {
    const m = S.mut();
    if (trip()) {
      if (!Answers.onLine(p)) { S.note = { text: `${short(p.name)} is not on the route.` }; return; }
      if (m.points.some((x) => x.place === p.id)) { S.note = { text: `${short(p.name)} is already a stop.` }; return; }
      commit((mm) => Trip.addPoint(mm, { id: 'v-' + p.id, name: p.name, kind: 'visit', km: Trip.tripKm(p), place: p.id }));
    } else {
      if (m.visits.some((v) => v.place === p.id)) { S.note = { text: `${short(p.name)} is already a stop.` }; return; }
      const k = DATA.ROUTES.import.krone, krone = S.plan().kind === 'import' && p.id === k.place;
      commit((mm) => mm.visits.push({ id: 'v-' + p.id, name: p.name, place: p.id, path: krone ? k.path : null, km: krone ? k.km : Math.round((p.off || 0) * 20) / 10, climb: krone ? k.climb : p.climb || 0 }));
    }
    S.note = { text: `${short(p.name)} added as a stop.`, undo: true };
  }
  // The point behind a list row or a map mark, wherever it lives.
  function findPoint(id) {
    const m = S.mut();
    if (trip()) { for (const d of Trip.days(m)) if (!d.rest) { const p = Trip.points(m, d).find((x) => x.id === id); if (p) return { p, day: d, pos: p.place && DATA.place(p.place).off ? DATA.place(p.place) : Trip.pointAt(p.km, MapView.view.map), meta: `${p.fixed ? (p.i === 0 ? 'Start' : 'Day end') : DATA.POINT_KINDS[p.kind].label} · km ${Math.round(p.km - d.start)} of Day ${d.n}` }; } return null; }
    const i = m.pts.findIndex((x) => x.id === id); if (i < 0) return null; const p = m.pts[i];
    return { p, i, pos: { map: 'bf', x: p.x, y: p.y }, meta: `${p.kind === 'start' ? 'Start' : DATA.POINT_KINDS[p.kind].label} · ${num(p.elev)} m` };
  }
  function openPoint(id) { const f = findPoint(id); if (!f) return; S.callout = { kind: 'point', pos: f.pos, point: f.p, idx: f.i, meta: f.meta }; showOnMap(f.pos); }

  // ---- actions (reached by data-act) ----
  const A = {
    selDay(d) { S.selDay = +d.n; frameDay(S.selDay); },
    expandDay(d) { const n = +d.n; if (S.expanded.has(n)) S.expanded.delete(n); else S.expanded.add(n); },
    dayMenu(d) { S.menuDay = +d.i; S.menu = 'day'; },
    dayEndAtStop(d) { const D = Trip.days(S.mut()); clearBox(); S.pointed = { type: 'day', day: D[+d.i].n, part: 'end' }; edit({ what: ['campsite', 'lodging'] }); S.edited = false; S.menu = null; run(); if (phone()) Sheet.setDetent('middle'); },
    dayRename(d) { S.renaming = +d.i; S.menu = null; if (phone()) Sheet.setDetent('middle'); },
    daySplit(d) { const n = Trip.days(S.mut())[+d.i].n; commit((m) => Trip.splitDay(m, +d.i)); S.note = { text: `Day ${n} split in two.`, undo: true }; S.menu = null; },
    dayJoin(d) { const n = Trip.days(S.mut())[+d.i].n; commit((m) => Trip.joinDay(m, +d.i)); S.note = { text: `Day ${n} joined with the next day.`, undo: true }; S.menu = null; S.selDay = null; },
    pointEnd(d) { S.pointed = { type: 'day', day: +d.n, part: 'end' }; S.pick = false; S.picker = null; S.callout = null; delete S.edits.where; run(); if (!phone()) Box.focus(); else Sheet.setDetent('middle'); },
    removePointed() { S.pointed = null; run(); },
    selRow(d) { S.selRow = d.id; const p = selectedPlace(); if (p && !String(d.id).startsWith('gap')) showOnMap(p); },
    selOpt(d) { S.selOpt = d.id; run(); },
    toggleMore() { S.moreOpen = !S.moreOpen; },
    pickField(d) { S.picker = S.picker === d.field ? null : d.field; },
    pickKind(d) { const k = S.req.kinds.slice(), i = k.indexOf(d.k); if (i >= 0) { if (k.length > 1) k.splice(i, 1); } else k.push(d.k); edit({ what: k }); run(); },
    pickWhere(d) { edit({ where: d.type === 'day' ? { type: 'day', day: +d.n, part: d.part || (S.req.where && S.req.where.day === +d.n ? S.req.where.part : 'whole') } : { type: d.type } }); S.frameKey = null; run(); },
    pickOnMap() { S.pick = true; S.picker = null; if (phone()) Sheet.setDetent('collapsed'); },
    cancelPick() { S.pick = false; },
    pickWithin(d) { edit({ within: +d.km }); run(); },
    wider(d) { edit({ within: +d.km }); run(); },
    pickOpen(d) { if (d.wd === 'any') { S.removed = { open: (S.ans && S.ans.open) || S.req.open || { wd: 'sun' } }; edit({ open: null }); S.picker = null; } else { S.removed = {}; edit({ open: { wd: d.wd } }); } run(); },
    removeFilter() { A.pickOpen({ wd: 'any' }); },
    restore() { edit({ open: S.removed.open }); S.removed = {}; run(); },
    pickBike(d) { if (S.req && S.req.kind === 'route') edit(d.bike ? { bike: d.bike } : { goal: d.goal }); else Object.assign(S.mut(), d.bike ? { bike: d.bike } : { goal: d.goal }); run(); },
    find(d) { edit({ what: d.k === 'sleep' ? ['campsite', 'lodging'] : [d.k] }); S.edited = false; run(); },
    setDates() { S.dates = true; S.menu = null; run(); },
    toggleDates() { S.dates = !S.dates; run(); },
    endDay() {
      const p = selectedPlace(); if (!p || !Answers.onLine(p)) return;
      const d = Trip.days(S.mut())[S.ans.endDay - 1];
      if (p.off > 0.05) { S.callout = { kind: 'offline', pos: p, place: p, day: d.n, i: d.i }; showOnMap(p); return; }
      applyEnd(d.i, p, null);
    },
    addStop() { const p = selectedPlace(); if (p) addStopPlace(p); },
    addToRoute() {
      const p = S.ans && S.ans.place; if (!p) return;
      if (trip()) { if (!Answers.onLine(p)) { S.note = { text: `${p.name} is not on the route.` }; return; } commit((m) => Trip.addPoint(m, { id: 'p-' + p.id, name: p.name, kind: 'pass', km: Trip.tripKm(p), place: p.id })); }
      else commit((m) => { Route.append(m, p); Object.assign(m.pts[m.pts.length - 1], { name: p.name, place: p.id, elev: p.elev }); });
      S.note = { text: `${p.name} added to the route as a pass-here point.`, undo: true };
    },
    routeFrom() { const p = S.ans && S.ans.place; if (!p) return; S.routeTo = p; S.frameKey = null; run(); },
    useRoute() { const a = S.ans; if (!a || a.kind !== 'route' || a.noData) return; commit((m) => Route.useOption(m, a.data, a.sel, a.id)); clearBox(); S.note = { text: `${a.data.title} · ${a.sel.name} · ${a.sel.km} km.`, undo: true }; frameAll(); },
    apply() {
      const a = S.ans; if (!a || a.kind !== 'change' || a.noData) return;
      if (a.type === 'dayend') applyEnd(a.i, a.place, a.place.off ? 'outback' : null);
      else { const k = DATA.ROUTES.import.krone; commit((m) => m.visits.push({ id: 'v-' + a.place.id, name: a.place.name, place: a.place.id, path: a.ghost, km: k.km, climb: k.climb })); clearBox(); S.note = { text: `${a.place.name} added as a visit.`, undo: true }; }
    },
    cancel() { clearBox(); },
    addMarker() { const a = S.ans, i = +String(S.selRow).replace('gap', ''), it = a && a.items[i]; if (!it || !it.start) return; const p = DATA.place(it.start); commit((m) => Trip.addPoint(m, { id: 'gap-' + it.start, name: it.title, kind: 'marker', km: Trip.tripKm(p), place: it.start })); S.note = { text: `Marker added at ${p.name}.`, undo: true }; },
    send() { const m = S.mut(); const what = trip() ? DATA.TRIP.facts : m.route ? `${DATA.ROUTES[m.route.id].title} · ${routeOption(m.route).name}` : m.legs.length ? `${km1(Route.figures(m).km)} km` : null; S.note = { text: what ? `Sent to OBC · ${what}.` : 'Nothing to send yet.' }; S.menu = null; },
    undo() { if (!S.hist.length) return; S.future.push(snapshot()); S.muts[S.planId] = JSON.parse(S.hist.pop()); S.note = null; S.menu = null; S.callout = null; if (S.text || S.pointed) run(); },
    redo() { if (!S.future.length) return; S.hist.push(snapshot()); S.muts[S.planId] = JSON.parse(S.future.pop()); S.note = null; S.menu = null; S.callout = null; if (S.text || S.pointed) run(); },
    clear() { clearBox(); },
    menu(d) { S.menu = S.menu === d.menu ? null : d.menu; S.picker = null; S.callout = null; },
    closeMenu() { S.menu = null; },
    // The plans page and the planner. Opening the same plan again keeps its edits.
    plans() { S.page = 'plans'; S.menu = null; S.callout = null; S.picker = null; if (S.focused) Box.input.blur(); },
    openPlan(d) {
      if (S.planId !== d.id) { S.planId = d.id; clearBox(); S.note = null; S.selDay = null; S.selRow = null; S.hist = []; S.future = []; S.callout = null; S.scrub = null; S.expanded.clear(); S.here = S.plan().here || S.here; MapView.setMap(S.plan().map); }
      S.page = 'planner'; S.menu = null; if (phone()) Sheet.setDetent('collapsed', false, true); frameAll(true);
    },
    search() { if (phone()) { S.prevDetent = Sheet.detent; Sheet.setFull(true); S.focused = true; render(); } Box.focus(); },
    theme() { const r = document.documentElement; r.dataset.theme = r.dataset.theme === 'dark' ? 'light' : 'dark'; },
    toggleFrame() { S.framed = !S.framed; S.rot = false; S.menu = null; layout(); },
    rotate() { S.rot = !S.rot; S.menu = null; layout(); },
    reset() { location.reload(); },
    zoomIn() { MapView.zoomBy(1.6); }, zoomOut() { MapView.zoomBy(1 / 1.6); }, fitAll() { frameAll(); },
    collapseProf() { S.profCollapsed = !S.profCollapsed; },
    quiet(d) { S.note = { text: d.text }; S.menu = null; },
    // the map callouts
    coClose() { S.callout = null; },
    coAdd() {
      const c = S.callout; if (!c) return;
      if (trip()) commit((m) => Trip.addPoint(m, { id: 'a' + Date.now(), name: c.name, kind: 'pass', km: c.km, place: c.placeId }));
      else commit((m) => Route.append(m, c.pos));
      S.callout = null; S.note = { text: `${c.name} added as a pass-here point.`, undo: true }; S.frameKey = null;
    },
    coEnd() { const c = S.callout; if (c.place.off > 0.05) { S.callout = { ...c, kind: 'offline' }; return; } applyEnd(c.i, c.place, null); },
    coVia(d) { const c = S.callout; applyEnd(c.i, c.place, d.via); },
    coStop() { addStopPlace(S.callout.place); S.callout = null; },
    coKind(d) {
      const c = S.callout, id = c.point.id;
      commit((m) => { if (trip()) Trip.setKind(m, id, d.kind); else m.pts.find((p) => p.id === id).kind = d.kind; });
      if (d.kind === 'sleep' && trip()) { S.callout = null; S.note = { text: `Day split at ${short(c.point.name)}.`, undo: true }; }
      else { c.point = { ...c.point, kind: d.kind }; S.note = { text: `${short(c.point.name)} is now ${DATA.POINT_KINDS[d.kind].label.toLowerCase()}.`, undo: true }; }
    },
    coRemove() { const c = S.callout; commit((m) => (trip() ? Trip.removePoint(m, c.point.id) : Route.remove(m, m.pts.findIndex((p) => p.id === c.point.id)))); S.callout = null; S.note = { text: `${short(c.point.name)} removed.`, undo: true }; },
    coMode(d) { const c = S.callout; setLegMode(c.key, c.leg, d.mode); if (d.mode !== 'drawn') c.mode = d.mode; },
    setMode(d) { setLegMode(S.menuKey, S.menuLeg, d.mode); S.menu = null; },
    legMenu(d) { S.menuKey = d.key; S.menuLeg = d.leg === '' ? null : +d.leg; S.menuMode = trip() ? S.mut().modes[d.key] || 'routed' : S.mut().legs[+d.leg].mode; S.menu = 'mode'; },
    cancelDraw() { S.drawing = null; MapView.setDraw(false); },
    pointCallout(d) { openPoint(d.pid); },
    pointTap(d) { openPoint(d.id); },
    legTap(d) {
      const m = S.mut();
      if (trip()) { const day = Trip.days(m)[+d.day], L = Trip.legs(m, day).find((x) => x.key === d.key); if (!L) return; S.callout = { kind: 'leg', pos: S.tapPos, key: L.key, leg: null, day: day.i, mode: L.mode, title: `${short(L.a.name)} → ${short(L.b.name)}`, meta: `Day ${day.n} · km ${Math.round(L.a.km - day.start)}–${Math.round(L.b.km - day.start)}` }; }
      else { const i = +d.key, L = m.legs[i], f = Route.legFigures(m, i); S.callout = { kind: 'leg', pos: S.tapPos, key: d.key, leg: i, mode: L.mode, fixed: !!L.fixed, title: `${short(m.pts[i].name)} → ${short(m.pts[i + 1].name)}`, meta: `${km1(f.km)} km · ${num(f.climb)} m${L.path ? '' : ' estimated'}` }; }
    },
    placeTap(d) {
      const p = DATA.place(d.id); let day = null, i = null;
      if (trip() && Answers.onLine(p)) { const D = Trip.days(S.mut()).filter((x) => !x.rest && x.i < S.mut().ends.length), km = Trip.tripKm(p), d0 = D.reduce((a, b) => (Math.abs(b.end - km) < Math.abs(a.end - km) ? b : a)); day = d0.n; i = d0.i; }
      S.callout = { kind: 'place', pos: p, place: p, day, i };
    },
  };
  // A leg's mode; Drawn hands the map to the rider until the stroke ends.
  function setLegMode(key, leg, mode) {
    if (mode === 'drawn') { S.drawing = { key, leg }; MapView.setDraw(true); S.callout = null; S.menu = null; if (phone()) Sheet.setDetent('collapsed'); return; }
    commit((m) => (trip() ? (m.modes[key] = mode) : Route.setMode(m, leg, mode)));
    S.note = { text: `Leg is now ${DATA.MODES[mode].toLowerCase()}.`, undo: true };
  }
  function endDraw(stroke) {
    const d = S.drawing; S.drawing = null; MapView.setDraw(false);
    if (stroke.length > 2) {
      commit((m) => { if (trip()) { m.strokes = m.strokes || {}; m.strokes[d.key] = { map: MapView.view.map, pts: stroke.map((q) => [q.x, q.y]) }; m.modes[d.key] = 'drawn'; } else Route.setDrawn(m, d.leg, stroke.map((q) => [q.x, q.y])); });
      S.note = { text: 'Leg drawn.', undo: true };
    }
  }
  // The profile's handlers: a handle drag moves a day end live; the scrubber reads the line.
  const profileHandlers = {
    onEnd(i, km, phase) {
      const m = S.mut();
      if (phase === 'tap') { A.pointEnd({ n: Trip.days(m)[i].n }); render(); return; }
      if (phase === 'move') { if (!S.dragEnd) S.dragEnd = snapshot(); Trip.setEnd(m, i, km); liveDays(); return m.ends[i]; }
      if (S.dragEnd) { S.hist.push(S.dragEnd); S.future = []; S.dragEnd = null; }
      S.note = null; render();
    },
    onScrub(km) { if (!trip()) return; S.scrub = Math.max(0, Math.min(Trip.TOTAL, km)); scrubRender(); },
    onTick(id) { A.selRow({ id }); render(); },
  };
  function liveDays() { Scene.renderMap(S); if (!S.show()) View.body(S, phone(), false); }
  let scrubRaf = null;
  function scrubRender() {
    if (scrubRaf) return;
    scrubRaf = requestAnimationFrame(() => {
      scrubRaf = null;
      const el = document.querySelector('#map-marks .scrubpt');
      if (el) { const c = MapView.convert(Trip.pointAt(S.scrub, MapView.view.map), MapView.view.map); el.style.transform = `translate(${c.x.toFixed(1)}px,${c.y.toFixed(1)}px) scale(var(--mk))`; } else Scene.renderMap(S);
      if (phone() && !S.land) View.head(S, true, S.show());
      Scene.renderProfile(S);
    });
  }

  // ---- render ----
  function render() {
    const ph = phone(), a = app(), show = S.show();
    a.className = `${ph ? 'phone' : 'web'} ${S.land ? 'land' : ''} ${S.pick ? 'picking' : ''} ${S.focused ? 'focused' : ''} ${S.drawing ? 'draw' : ''}`;
    $('stage').className = `${S.framed ? 'framed' : ''} ${S.rot ? 'rot' : ''}`;
    $('frame-tools').hidden = !S.framed;
    View.plans(S);
    if (hasLine()) S.win = Scene.window(S);
    if (S.land) { View.land(S); Scene.renderProfile(S); View.menus(S, true); return; }
    View.bar(S); View.nav(S); Box.sync(S); $('qcancel').hidden = !(ph && S.focused);
    View.chips(S, ph); View.head(S, ph, show); View.body(S, ph, show); View.pin(S, ph, show); View.foot(S);
    Scene.renderMap(S); Scene.renderProfile(S); View.callout(S); View.menus(S, ph);
    if (ph) { Sheet.measure(a.clientHeight); const pick = $('chips').querySelector('.pick'); Sheet.setExtra(pick && !S.focused ? pick.offsetHeight + 10 : 0); }
  }

  // ---- layout and wiring ----
  function layout() {
    const w = innerWidth, h = innerHeight;
    S.layout = S.framed || w < 700 || h < 500 ? 'phone' : 'web';
    S.land = phone() && (S.framed ? S.rot : w > h);
    if (!phone()) { app().style.height = ''; $('panel').style.height = ''; }
    render(); frameAll(true); render();
  }
  const ANCHORED = ['dayMenu', 'legMenu'];
  function onClick(e) {
    if (S.pressedUntil > performance.now()) { S.pressedUntil = 0; return; }   // the click after a long press
    const t = e.target.closest('[data-act]');
    if (!t || t.disabled || t.classList.contains('dis')) { if (!e.target.closest('#chips, .pick, .menu, #callout') && S.picker) { S.picker = null; render(); } return; }
    if (!(t.dataset.act in A)) return;
    if (t.dataset.act === 'pickField' && t.closest('.pick')) return;
    if (t.dataset.act === 'selDay' && e.target.closest('.dmore, .dchev, input')) return;
    S.menuAnchor = ANCHORED.includes(t.dataset.act) ? t.getBoundingClientRect() : null;
    A[t.dataset.act](t.dataset);
    if (!['pickField', 'pickKind', 'pickWhere', 'pickWithin', 'pickOpen', 'pickBike', 'restore', 'toggleMore', 'menu', 'closeMenu', 'toggleDates'].includes(t.dataset.act) && !t.closest('.pick')) S.picker = null;
    if (!['menu', 'pickBike', 'toggleDates', 'dayMenu', 'legMenu'].includes(t.dataset.act)) S.menu = null;
    render();
  }
  function init() {
    pf = $('pf'); Trip.init();
    MapView.insets = () => S.insets();
    MapView.init($('map'), {
      tap: (d, pos) => { if (d.act in A) { S.tapPos = pos; S.menuAnchor = null; A[d.act](d); S.menu = null; render(); } },
      tapEmpty: (pos) => {
        if (S.menu || S.picker || S.callout) { S.menu = null; S.picker = null; S.callout = null; render(); return; }
        if (S.focused || S.pick || S.drawing || (S.show() && S.ans.kind === 'route')) return;
        if (trip()) { const n = Line.nearest(Trip.line(pos.map), pos), m = DATA.MAPS[pos.map], far = n.dist * m.kmx > 4; if (far) return; const name = Trip.endName(n.km); S.callout = { kind: 'add', pos, km: n.km, name: name.startsWith('km') ? `Near ${name}` : `Near ${name}`, meta: `Day ${Trip.dayAt(Trip.days(S.mut()), n.km).n} · km ${Math.round(n.km - Trip.dayAt(Trip.days(S.mut()), n.km).start)}`, placeId: null }; }
        else { const nr = Route.near(pos); S.callout = { kind: 'add', pos, name: nr.d <= 2.5 ? nr.p.name : `Point ${S.mut().pts.length + 1}`, meta: nr.d > 2.5 ? `${km1(nr.d)} km from ${nr.p.name}` : `${num(nr.p.elev)} m` }; }
        render();
      },
      change: () => render(),
      move: () => Scene.onMove(S),
      legDrag: (i, pos, phase) => {
        if (trip() || S.mut().legs[i].fixed) return;
        if (phase === 'start') { commit((m) => Route.insert(m, i, pos)); S.dragPt = i + 1; S.callout = null; Scene.renderMap(S); return; }
        Route.move(S.mut(), S.dragPt, pos);
        if (phase === 'end') { S.dragPt = null; S.frameKey = null; S.note = { text: 'Point added in the leg.', undo: true }; render(); } else Scene.renderMap(S);
      },
      draw: (stroke, phase) => { if (!S.drawing) return; if (phase === 'end') { endDraw(stroke); render(); } else Scene.drawPreview(stroke); },
    });
    Sheet.init($('panel'), { onDetent: () => render() });
    Box.init({
      onInput: (v) => { S.text = v.slice(0, 80); S.edits = {}; S.removed = {}; S.edited = false; S.submitted = false; S.routeTo = null; S.picker = null; S.note = null; if (!S.text && !S.pointed) { clearBox(); render(); return; } run(); render(); },
      onSubmit: () => { S.submitted = true; run(); render(); },
      onClear: () => { clearBox(); render(); },
      onFocus: () => { S.focused = true; S.callout = null; S.menu = null; if (phone() && !S.land && !Sheet.full) { S.prevDetent = Sheet.detent; Sheet.setFull(true); } render(); },
      onBlur: () => { S.focused = false; Box.sync(S); setTimeout(() => { if (S.focused) return; if (phone() && Sheet.full) { Sheet.setFull(false); Sheet.setDetent(S.show() ? 'middle' : S.prevDetent, true, true); } render(); }, 80); },
      onRemovePointed: () => { A.removePointed(); render(); },
    });
    $('qcancel').addEventListener('click', () => { clearBox(); Box.input.blur(); render(); });
    $('stage').addEventListener('click', onClick);
    $('body').addEventListener('touchmove', () => { if (S.focused) Box.input.blur(); }, { passive: true });
    // a long press or a right click on a day row opens its actions
    let press = null;
    $('body').addEventListener('touchstart', (e) => { const row = e.target.closest('.day'); if (!row) return; press = setTimeout(() => { S.menuAnchor = row.getBoundingClientRect(); A.dayMenu({ i: row.dataset.i }); render(); press = null; S.pressedUntil = performance.now() + 1500; }, 500); }, { passive: true });
    for (const ev of ['touchmove', 'touchend', 'touchcancel']) $('body').addEventListener(ev, () => { if (press) { clearTimeout(press); press = null; } }, { passive: true });
    $('body').addEventListener('contextmenu', (e) => { const row = e.target.closest('.day'); if (!row) return; e.preventDefault(); S.menuAnchor = row.getBoundingClientRect(); A.dayMenu({ i: row.dataset.i }); render(); });
    $('body').addEventListener('keydown', (e) => { const inp = e.target.closest('input.rename'); if (!inp) return; if (e.key === 'Enter') { commit((m) => { const v = inp.value.trim(); if (v) m.names[+inp.dataset.i] = v; else delete m.names[+inp.dataset.i]; }); S.renaming = null; render(); } if (e.key === 'Escape') { S.renaming = null; render(); } });
    $('body').addEventListener('focusout', (e) => { if (e.target.closest && e.target.closest('input.rename') && S.renaming != null) { const inp = e.target, v = inp.value.trim(); commit((m) => { if (v) m.names[+inp.dataset.i] = v; else delete m.names[+inp.dataset.i]; }); S.renaming = null; setTimeout(render, 0); } });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && (S.menu || S.picker || S.pick || S.callout || S.drawing)) { S.menu = null; S.picker = null; S.pick = false; S.callout = null; A.cancelDraw(); render(); }
      if ((e.metaKey || e.ctrlKey) && e.key === 'z' && document.activeElement !== Box.input) { e.preventDefault(); e.shiftKey ? A.redo() : A.undo(); render(); }
    });
    let lastOrient = innerWidth > innerHeight;
    addEventListener('resize', () => { const o = innerWidth > innerHeight, mode = S.framed || innerWidth < 700 || innerHeight < 500 ? 'phone' : 'web'; if (o !== lastOrient || mode !== S.layout) { lastOrient = o; layout(); } else render(); });
    if (window.visualViewport) visualViewport.addEventListener('resize', () => { if (phone() && !S.framed && !S.land) { app().style.height = visualViewport.height + 'px'; scrollTo(0, 0); render(); } });
    document.addEventListener('gesturestart', (e) => e.preventDefault());
    layout();
  }
  return { init, S, render, A, routeOption, hasLine, profileHandlers, get pf() { return pf; } };
})();
document.addEventListener('DOMContentLoaded', App.init);
