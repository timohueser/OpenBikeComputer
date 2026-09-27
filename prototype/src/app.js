/* The app: one state object, undo/redo as snapshots of the plan's mutable part, an actions table
   reached by data-act, and one render pass that rebuilds the panel, the map layers, the profile
   and the menus. The website and the phone share the DOM; CSS lays it out by S.layout. */

const App = (() => {
  const { esc, icon, num, hm, km1 } = Answers;
  const $ = (id) => document.getElementById(id);
  const freshMut = () => ({ d4End: 104, points: [], route: null, bike: null, goal: null });
  const S = {
    layout: 'web', framed: false, planId: 'alps', muts: {}, text: '', pointed: null, edits: {}, removed: {}, edited: false, focused: false, submitted: false,
    req: null, ans: null, routeTo: null, selDay: null, selRow: null, selOpt: 'shortest', picker: null, note: null, menu: null, menuAnchor: null,
    dates: true, today: 3, pick: false, moreOpen: false, hist: [], future: [], profCollapsed: false, here: 'glottertal', frameKey: null,
    plan() { return DATA.PLANS.find((p) => p.id === S.planId); },
    mut() { return S.muts[S.planId] || (S.muts[S.planId] = freshMut()); },
    preset() { const m = S.mut(), p = S.plan(); return { bike: m.bike || p.bike, goal: m.goal || p.goal }; },
    insets() { return phone() ? { t: navH(), b: Math.min(Sheet.height, Sheet.heights.medium) } : { t: 0, b: 0 }; },
  };
  const phone = () => S.layout === 'phone';
  const navH = () => ($('nav').offsetHeight || 0);
  const app = () => $('app');
  const SHOW = () => S.ans && (S.text || S.pointed || S.routeTo);

  // ---- the request → answer pipeline ----
  function run() {
    const plan = S.plan();
    const ctx = { plan, hasDates: S.dates && plan.kind === 'trip', today: S.today, selDay: S.selDay, pointed: S.pointed, region: plan.map === 'bf' ? 'Black Forest' : 'Alps', map: plan.map };
    let req = Parser.parse(S.text, ctx);
    if (S.routeTo) req = { ...req, kind: 'route', to: S.routeTo, off: [], bike: S.preset().bike, goal: S.preset().goal };
    const E = S.edits;
    if (E.what && (req.kind === 'places' || (req.kind === 'none' && !S.text))) { req.kinds = E.what; req.kind = 'places'; }
    if (req.kind === 'places') {
      req.where = E.where || Parser.where(req, ctx);
      if (E.within != null) req.within = E.within;
      if ('open' in E) req.open = E.open;
    }
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
  // Frame the map once per new answer: the day of the where, the route, the stretch, the place.
  const dayTok = (n) => { const i = DATA.DAYS.filter((d) => !d.rest).findIndex((d) => d.n === n); return i < 0 ? null : `var(--day-${(i % 4) + 1})`; };
  const answerKey = (a) => !a || a.noData ? null : a.kind === 'places' ? `places:${JSON.stringify(a.where)}` : a.kind === 'route' ? `route:${a.to.id}` : a.kind === 'change' ? `change:${a.type}` : a.kind === 'gaps' ? `gaps:${a.gapKind}` : `place:${a.place && a.place.id}`;
  function autoFrame() {
    const a = S.ans, key = answerKey(a); if (!key) return;
    if (key === S.frameKey) return; S.frameKey = key;
    if (a.kind === 'places' && a.where.type === 'day') { S.selDay = a.where.day; frameDay(a.where.day); }
    else if (a.kind === 'places' && a.where.type === 'route') frameAll();
    else if (a.kind === 'places' && a.where.type === 'near') MapView.panTo(a.where.place, S.insets());
    else if (a.kind === 'route') frameBox(a.data.options.map((o) => MapView.bboxOfPath(o.path)).reduce(MapView.union), 'bf');
    else if (a.kind === 'change' && a.type === 'dayend') { S.selDay = 4; frameDay(4); }
    else if (a.kind === 'change' && a.type === 'visit') frameBox(MapView.bboxOfPath('route-bf-import'), 'bf');
    else if (a.kind === 'gaps' && a.items.length) { if (a.gapKind === 'water' && S.plan().kind === 'trip') { S.selDay = 4; frameDay(4); } else frameAll(); }
    else if ((a.kind === 'place' || a.kind === 'none') && a.place && a.place.map !== 'bf' === (S.plan().map !== 'bf')) MapView.panTo(a.place, S.insets());
  }
  const frameBox = (box, map, opts) => MapView.frame(map, box, S.insets(), Object.assign({ pad: 50 }, opts));
  function frameDay(n) {
    if (n === 4) frameBox(MapView.bboxOfPath('route-d4'), 'd4');
    else frameBox(MapView.bboxOfPath(`route-alps-d${n}`), 'alps', { max: 2.1, pad: 70 });
  }
  function frameAll(instant) {
    const plan = S.plan(), mut = S.mut();
    if (plan.kind === 'trip') frameBox(DATA.DAYS.filter((d) => !d.rest).map((d) => MapView.bboxOfPath(`route-alps-d${d.n}`)).reduce(MapView.union), 'alps', { pad: 30, instant });
    else if (plan.kind === 'import') frameBox(MapView.bboxOfPath('route-bf-import'), 'bf', { instant });
    else if (mut.route) frameBox(MapView.bboxOfPath(routeOption(mut.route).path), 'bf', { instant });
    else frameBox({ x: 150, y: 60, w: 760, h: 600 }, 'bf', { instant, pad: 10 });
  }
  const routeOption = (r) => DATA.ROUTES[r.id].options.find((o) => o.id === r.opt);
  const snapshot = () => JSON.stringify(S.mut());
  function commit(fn) { S.hist.push(snapshot()); S.future = []; fn(S.mut()); }
  function clearBox() { S.text = ''; S.pointed = null; S.edits = {}; S.removed = {}; S.edited = false; S.submitted = false; S.routeTo = null; S.picker = null; S.pick = false; S.moreOpen = false; S.ans = null; S.req = null; S.frameKey = null; }
  const selectedPlace = () => { const a = S.ans; if (!a) return null; const r = a.rows && a.rows.find((r) => r.p.id === S.selRow); return r ? r.p : a.empty && a.empty.nearest && a.empty.nearest.p.id === S.selRow ? a.empty.nearest.p : a.place || null; };
  const edit = (patch) => { Object.assign(S.edits, patch); S.edited = true; };

  // ---- actions (reached by data-act) ----
  const A = {
    selDay(d) { S.selDay = +d.n; frameDay(S.selDay); },
    pointEnd(d) { S.pointed = { type: 'day', day: +d.n, part: 'end' }; S.pick = false; S.picker = null; delete S.edits.where; run(); if (!phone() || Sheet.detent !== 'high') Box.focus(); if (phone()) Sheet.setDetent('medium'); },
    removePointed() { S.pointed = null; run(); },
    selRow(d) { S.selRow = d.id; const p = selectedPlace(); if (p && !String(d.id).startsWith('gap') && !MapView.inRect(p, MapView.viewRect(S.insets()))) MapView.panTo(p, S.insets()); },
    selOpt(d) { S.selOpt = d.id; run(); },
    toggleMore() { S.moreOpen = !S.moreOpen; },
    pickField(d) { S.picker = S.picker === d.field ? null : d.field; },
    pickKind(d) { const k = S.req.kinds.slice(), i = k.indexOf(d.k); if (i >= 0) { if (k.length > 1) k.splice(i, 1); } else k.push(d.k); edit({ what: k }); run(); },
    pickWhere(d) { edit({ where: d.type === 'day' ? { type: 'day', day: +d.n, part: d.part || (S.req.where && S.req.where.day === +d.n ? S.req.where.part : 'whole') } : { type: d.type } }); S.frameKey = null; run(); },
    pickOnMap() { S.pick = true; S.picker = null; if (phone()) Sheet.setDetent('low'); },
    pickWithin(d) { edit({ within: +d.km }); run(); },
    wider(d) { edit({ within: +d.km }); run(); },
    pickOpen(d) { if (d.wd === 'any') { S.removed = { open: (S.ans && S.ans.open) || S.req.open || { wd: 'sun' } }; edit({ open: null }); S.picker = null; } else { S.removed = {}; edit({ open: { wd: d.wd } }); } run(); },
    removeFilter() { A.pickOpen({ wd: 'any' }); },
    restore() { edit({ open: S.removed.open }); S.removed = {}; run(); },
    pickBike(d) { if (S.req && S.req.kind === 'route') edit(d.bike ? { bike: d.bike } : { goal: d.goal }); else Object.assign(S.mut(), d.bike ? { bike: d.bike } : { goal: d.goal }); run(); },
    find(d) { edit({ what: d.k === 'sleep' ? ['campsite', 'lodging'] : [d.k] }); S.edited = false; run(); },
    setDates() { S.dates = true; S.menu = null; run(); },
    toggleDates() { S.dates = !S.dates; run(); },
    endDay() { const p = selectedPlace(); if (!p || p.km == null) return; const day = S.ans.endDay || 4; commit((m) => { m.d4End = p.km; }); const name = Answers.endName(p.km); clearBox(); S.note = { text: `Day ${day} now ends at ${name}.`, undo: true }; S.selDay = day; frameDay(day); },
    addStop() { const p = selectedPlace(); if (!p) return; if (S.mut().points.some((x) => x.id === p.id)) { S.note = { text: `${p.name} is already a stop.` }; return; } commit((m) => m.points.push({ id: p.id, kind: 'visit', name: p.name })); S.note = { text: `${p.name} added as a stop.`, undo: true }; },
    addToRoute() { const p = S.ans && S.ans.place; if (!p) return; commit((m) => m.points.push({ id: p.id, kind: 'pass', name: p.name })); S.note = { text: `${p.name} added to the route as a pass-here point.`, undo: true }; },
    routeFrom() { const p = S.ans && S.ans.place; if (!p) return; S.routeTo = p; S.frameKey = null; run(); },
    apply() {
      const a = S.ans; if (!a || a.kind !== 'change' || a.noData) return;
      if (a.type === 'dayend') { commit((m) => { m.d4End = a.km; }); clearBox(); S.note = { text: `Day 4 now ends at ${a.place.name}.`, undo: true }; S.selDay = 4; }
      else { commit((m) => m.points.push({ id: a.place.id, kind: 'visit', name: a.place.name, path: a.ghost })); clearBox(); S.note = { text: `${a.place.name} added as a visit.`, undo: true }; }
    },
    cancel() { clearBox(); },
    addMarker() { const a = S.ans, i = +String(S.selRow).replace('gap', ''), it = a && a.items[i]; if (!it || !it.start) return; const p = DATA.place(it.start); commit((m) => m.points.push({ id: 'gap-' + it.start, kind: 'marker', name: it.title, place: it.start })); S.note = { text: `Marker added at ${p.name}.`, undo: true }; },
    send() { const a = S.ans; if (a && a.kind === 'route' && !a.noData) { commit((m) => { m.route = { id: Object.keys(DATA.ROUTES).find((k) => DATA.ROUTES[k] === a.data), opt: a.sel.id }; }); S.note = { text: `Sent to OBC · ${a.data.title} · ${a.sel.name} · ${a.sel.km} km.` }; } else S.note = { text: 'Sent to OBC.' }; S.menu = null; },
    undo() { if (!S.hist.length) return; S.future.push(snapshot()); S.muts[S.planId] = JSON.parse(S.hist.pop()); S.note = null; S.menu = null; if (S.text || S.pointed) run(); },
    redo() { if (!S.future.length) return; S.hist.push(snapshot()); S.muts[S.planId] = JSON.parse(S.future.pop()); S.note = null; S.menu = null; if (S.text || S.pointed) run(); },
    clear() { clearBox(); },
    menu(d) { S.menu = S.menu === d.menu ? null : d.menu; S.picker = null; },
    closeMenu() { S.menu = null; },
    switchPlan(d) { if (S.planId !== d.id) { S.planId = d.id; clearBox(); S.note = null; S.selDay = null; S.selRow = null; S.hist = []; S.future = []; S.here = S.plan().here || S.here; MapView.setMap(S.plan().map); frameAll(true); } S.menu = null; },
    toggleFrame() { S.framed = !S.framed; S.menu = null; layout(); },
    reset() { location.reload(); },
    zoomIn() { MapView.zoomBy(1.6); }, zoomOut() { MapView.zoomBy(1 / 1.6); }, fitAll() { frameAll(); },
    collapseProf() { S.profCollapsed = !S.profCollapsed; },
    quiet(d) { S.note = { text: d.text }; S.menu = null; },
  };

  // ---- render ----
  function render() {
    const ph = phone(), a = app();
    a.className = `mk ${ph ? 'mk-phone phone' : 'web'} ${S.framed ? 'framed' : ''} ${S.pick ? 'picking' : ''}`;
    $('stage').classList.toggle('framed', S.framed);
    renderBar(); renderNav(); Box.sync(S); renderChips(); renderOneline(); renderBody(); renderPin(); renderFoot(); renderMap(); renderProfile(); renderMenus();
    if (ph) Sheet.measure(a.clientHeight, navH());
  }
  const btn = (act, label, ic, cls = 'ghost', extra = '') => `<button class="btn ${cls}" data-act="${act}" ${extra}>${ic ? icon(ic) : ''}${label ? `<span>${label}</span>` : ''}</button>`;
  function renderBar() {
    const p = S.plan(), trip = p.kind === 'trip', pr = S.preset();
    $('bar').innerHTML = `<span class="brand">OBC Planner</span>
      <button class="plan" data-act="menu" data-menu="plan">${esc(p.name)}${icon('i-chev-d', 'chev')}</button>
      ${trip ? `<button class="dates" data-act="menu" data-menu="dates">${icon('i-calendar')}${S.dates ? DATA.TRIP.datesShort : 'No dates'}</button>` : ''}
      <button class="preset" data-act="menu" data-menu="bike" title="Bike and goal">${icon('i-bike')}${DATA.BIKES[pr.bike]} · ${DATA.GOALS[pr.goal]}${icon('i-chev-d', 'chev')}</button>
      <button class="proto" data-act="menu" data-menu="proto">Prototype · example data</button>
      <div class="tools">${btn('undo', '', 'i-undo', 'ghost icon', `title="Undo" ${S.hist.length ? '' : 'disabled'}`)}${btn('redo', '', 'i-redo', 'ghost icon', `title="Redo" ${S.future.length ? '' : 'disabled'}`)}<span class="sep"></span>
      ${btn('menu', 'Versions', 'i-versions', 'ghost', 'data-menu="versions"')}${btn('menu', 'Import', 'i-import', 'ghost', 'data-menu="import"')}${btn('menu', 'Offline areas', 'i-download', 'ghost', 'data-menu="offline"')}${btn('menu', 'Continue on phone', 'i-qr', 'ghost', 'data-menu="qr"')}${btn('quiet', 'Export GPX', 'i-route', 'ghost', 'data-text="GPX export is not part of the prototype."')}<span class="sep"></span>
      ${btn('send', 'Send to device', 'i-send', 'primary')}</div>`;
  }
  function renderNav() {
    const p = S.plan();
    $('nav').innerHTML = `<button class="nb" data-act="menu" data-menu="plan">${icon('i-chev-l')}${p.kind === 'trip' ? 'Trips' : 'Routes'}</button>
      <button class="title" data-act="menu" data-menu="plan">${esc(p.name)}</button>
      <span class="nr"><button class="nb icon ${S.hist.length ? '' : 'dis'}" data-act="undo" title="Undo">${icon('i-undo')}</button><button class="nb icon" data-act="menu" data-menu="more" title="More">${icon('i-more')}</button></span>`;
  }
  function renderChips() {
    const c = $('chips');
    let h = '';
    if (S.req && (S.text || S.routeTo || S.edits.what)) h = Box.chipsHtml(Box.chips(S.req, S.ans, S), S) + Box.pickerHtml(S.picker, S.req, S.ans, S);
    else if (S.pointed) h = Box.findHtml() + (S.picker === 'where' ? Box.pickerHtml('where', null, null, S) : '');
    if (S.pick) h += `<div class="note quiet pickhint">Tap a day end on the map${phone() ? '' : ' or the profile'}.<button class="btn sm" data-act="cancelPick">Cancel</button></div>`;
    c.innerHTML = h; c.hidden = !h; Box.placeArrow(c);
  }
  A.cancelPick = () => { S.pick = false; };
  function restLine() {
    const p = S.plan(), m = S.mut();
    if (p.kind === 'trip') return `<b>10 days · ${DATA.TRIP.facts}</b><span class="cap">${S.dates ? DATA.TRIP.dates + ' · ' : ''}${num(DATA.TRIP.climb)} m · ${DATA.TRIP.unknown}</span>`;
    if (p.kind === 'import') return `<b>${DATA.ROUTES.import.facts}</b><span class="cap">${DATA.ROUTES.import.notes[0]}</span>`;
    if (m.route) { const o = routeOption(m.route); return `<b>${DATA.ROUTES[m.route.id].title} · ${o.km} km</b><span class="cap">${num(o.climb)} m · ${hm(o.time)} · ${DATA.BIKES[S.preset().bike]}</span>`; }
    return `<b>New route</b><span class="cap">From here · ${DATA.place(S.here).name} · ${DATA.BIKES[S.preset().bike]} · ${DATA.GOALS[S.preset().goal]}</span>`;
  }
  function renderOneline() {
    const head = SHOW() ? Answers.head(S.ans) : null;
    $('oneline').innerHTML = SHOW() ? (phone() ? (head ? `<b>${esc(head.b)}</b><span>${esc(head.span)}</span>` : '') : `${btn('clear', S.plan().kind === 'trip' ? 'Back to the days' : 'Back', 'i-chev-l', 'ghost sm')}<span class="cap">${head ? esc(head.web || head.b + (head.span ? ' · ' + head.span : '')) : ''}</span>`) : restLine();
    $('oneline').className = 'oneline ' + (SHOW() ? 'ans' : 'rest');
  }
  function daysHtml() {
    const D = Answers.days(S.mut()), max = Math.max(...D.map((d) => d.km));
    return D.map((d) => d.rest ? `<div class="day" data-act="selDay" data-n="${d.n}"><b class="n rest">${d.n}</b><div class="t">Rest day · ${esc(d.to)}</div><span class="km">–</span><div class="m">${S.dates ? d.date : ''}</div></div>`
      : `<div class="day ${S.selDay === d.n ? 'sel' : ''}" data-act="selDay" data-n="${d.n}" style="--dc:${dayTok(d.n)}"><b class="n">${d.n}</b><div class="t">${esc(d.from)} → ${esc(d.to)}</div><span class="km">${d.km} km</span><div class="m">${S.dates ? d.date + ' · ' : ''}${hm(d.time)} · ${num(d.climb)} m</div><i class="bar" style="width:calc((100% - 42px) * ${(d.km / max).toFixed(3)})"></i></div>`).join('');
  }
  function pointsHtml() {
    const pts = S.mut().points; if (!pts.length) return '';
    const ic = { visit: 'i-flag', pass: 'i-ring', marker: 'i-drop' }, kl = { visit: 'Visit', pass: 'Pass here', marker: 'Marker' };
    return `<div class="sec">Points</div>` + pts.map((p) => `<div class="row prow"><span class="ic">${icon(ic[p.kind])}</span><div><div class="t">${esc(p.name)}</div><div class="m">${kl[p.kind]}${p.place ? ` · ${esc(DATA.place(p.place).name)}` : ''}</div></div></div>`).join('');
  }
  function restHtml() {
    const p = S.plan(), m = S.mut();
    if (p.kind === 'trip') return daysHtml() + pointsHtml();
    if (p.kind === 'import') { const r = DATA.ROUTES.import; return `<dl class="ledger"><dt>Distance</dt><dd>${r.km} km</dd><dt>Climb</dt><dd>${num(r.climb)} m</dd><dt>Bike</dt><dd>Gravel</dd><dt>Surface</dt><dd>41 km <small>unknown</small></dd></dl>${r.notes.map((n) => `<div class="note quiet">${n}</div>`).join('')}` + pointsHtml(); }
    if (m.route) { const o = routeOption(m.route); return `<dl class="ledger"><dt>Route</dt><dd>${DATA.ROUTES[m.route.id].title}</dd><dt>Option</dt><dd>${o.name}</dd><dt>Distance</dt><dd>${o.km} km</dd><dt>Climb</dt><dd>${num(o.climb)} m</dd><dt>Riding time</dt><dd>${hm(o.time)} <small>estimated</small></dd></dl>` + pointsHtml(); }
    return `<dl class="ledger"><dt>Start</dt><dd>Here · ${esc(DATA.place(S.here).name)}</dd><dt>Bike</dt><dd>${DATA.BIKES[S.preset().bike]} · ${DATA.GOALS[S.preset().goal]}</dd></dl>` + pointsHtml();
  }
  function renderBody() {
    const note = S.note ? `<div class="note">${esc(S.note.text)}${S.note.undo && S.hist.length ? `<button class="btn sm" data-act="undo">${icon('i-undo')}Undo</button>` : ''}</div>` : '';
    $('body').innerHTML = note + (SHOW() ? Answers.body(S.ans, S, phone()) : restHtml());
  }
  function renderPin() {
    let acts = null;
    if (SHOW() && S.ans && !S.ans.noData) acts = S.ans.actions;
    else if (!SHOW() && phone() && (S.plan().kind !== 'new' || S.mut().route)) acts = { primary: { act: 'send', label: 'Send to device', icon: 'i-send' } };
    $('pin').innerHTML = acts ? `${acts.secondary ? btn(acts.secondary.act, acts.secondary.label, null, 'ghost') : ''}${btn(acts.primary.act, acts.primary.label, acts.primary.icon, 'primary')}` : '';
    $('pin').hidden = !acts;
  }
  function renderFoot() {
    const p = S.plan(), facts = p.kind === 'trip' ? `${DATA.TRIP.facts} · ${DATA.TRIP.unknown}` : p.kind === 'import' ? DATA.ROUTES.import.notes.slice(0, 2).join(' · ') : '';
    $('foot').innerHTML = facts ? `<span class="cap">${facts}</span>` : ''; $('foot').hidden = !facts;
  }

  // ---- map layers ----
  const mk = (pos, o) => {
    const c = MapView.convert(pos, MapView.view.map), r = o.r || 12, ir = o.ir || 7, act = o.act ? `data-act="${o.act}" data-id="${esc(o.id || '')}" data-n="${o.n || ''}"` : '';
    return `<g class="mk ${o.cls || ''}" style="transform:translate(${c.x.toFixed(1)}px,${c.y.toFixed(1)}px) scale(var(--mk));${o.style || ''}" ${act}><circle r="22" class="hit"/><circle r="${r}" class="ring"/>${o.icon ? `<use href="#${o.icon}" x="${-ir}" y="${-ir}" width="${ir * 2}" height="${ir * 2}"/>` : ''}${o.text ? `<text class="num" y="4.5">${o.text}</text>` : ''}${o.label ? `<text class="ml" x="${r + 6}" y="5">${esc(o.label)}</text>` : ''}</g>`;
  };
  const txt = (map, x, y, label, cls = '', fill = '') => { const c = MapView.convert({ map, x, y }, MapView.view.map); return `<g class="mk" style="transform:translate(${c.x}px,${c.y}px) scale(var(--mk))"><text class="ml ${cls}" text-anchor="middle" ${fill ? `style="fill:${fill}"` : ''}>${esc(label)}</text></g>`; };
  const line = (path, cls, act) => `<use href="#${path}" class="${cls}"/>` + (act ? `<use href="#${path}" class="rt-hit" ${act}/>` : '');
  function renderMap() {
    const plan = S.plan(), mut = S.mut(), map = MapView.view.map, a = SHOW() ? S.ans : null;
    let routes = '', marks = '';
    if (plan.kind === 'trip') {
      if (map === 'alps') {
        const days = DATA.DAYS.filter((d) => !d.rest), dim = (n) => (S.selDay && S.selDay !== n ? 'dim' : '');
        routes = days.map((d) => `<use href="#route-alps-d${d.n}" class="rt-casing ${dim(d.n)}"/>`).join('') + days.map((d) => `<use href="#route-alps-d${d.n}" class="rt-line ${dim(d.n)}" style="stroke:${dayTok(d.n)}"/>`).join('') + days.map((d) => `<use href="#route-alps-d${d.n}" class="rt-hit" data-act="selDay" data-n="${d.n}"/>`).join('');
        marks += mk({ map: 'alps', x: 190, y: 32 }, { cls: 'start', r: 9 });
        for (const d of days) { const pos = d.n === 4 ? MapView.d4Point(mut.d4End) : { map: 'alps', x: DATA.DAY_ENDS_ALPS[d.n][0], y: DATA.DAY_ENDS_ALPS[d.n][1] }; marks += mk(pos, { cls: 'dend' + (S.pointed && S.pointed.day === d.n ? ' pointed' : ''), r: 13, text: d.n, act: 'pointEnd', n: d.n, style: `--dc:${dayTok(d.n)}` }); }
      } else {
        const D = Answers.days(mut)[3];
        routes = line('route-d4', 'rt-casing') + `<use href="#route-d4" class="rt-line" style="stroke:${dayTok(4)}"/><use href="#route-d4" class="rt-hit" data-act="selDay" data-n="4"/>`;
        marks += txt('d4', 520, 372, `Day 4 · ${D.km} km · ${hm(D.time)}`, '', dayTok(4));
        marks += mk({ map: 'd4', x: 840, y: 78 }, { cls: 'dend' + (S.pointed && S.pointed.day === 3 ? ' pointed' : ''), r: 12, icon: 'i-tent', act: 'pointEnd', n: 3, label: 'End of Day 3', style: `--dc:${dayTok(3)}` });
        const hide = a && a.kind === 'change' && a.type === 'dayend';
        if (!hide) marks += mk(MapView.d4Point(mut.d4End), { cls: 'dend' + (S.pointed && S.pointed.day === 4 ? ' pointed' : ''), r: 12, icon: 'i-tent', act: 'pointEnd', n: 4, label: `End of Day 4 · ${Answers.endName(mut.d4End)}`, style: `--dc:${dayTok(4)}` });
      }
    } else if (plan.kind === 'new') {
      if (a && a.kind === 'route' && !a.noData) {
        routes = a.options.filter((o) => o !== a.sel).map((o) => line(o.path, 'rt-alt-casing') + line(o.path, 'rt-alt', `data-act="selOpt" data-id="${o.id}"`)).join('') + line(a.sel.path, 'rt-casing') + line(a.sel.path, 'rt-line');
        marks += a.options.filter((o) => o.label).map((o) => txt('bf', o.label[1], o.label[2], o.label[0], o === a.sel ? 'rt' : '')).join('');
      } else if (mut.route) { const o = routeOption(mut.route); routes = line(o.path, 'rt-casing') + line(o.path, 'rt-line'); marks += mk(DATA.place(DATA.ROUTES[mut.route.id].to), { cls: 'dest', r: 13, icon: 'i-flag', ir: 8 }); }
      marks += mk(DATA.place(S.here), { cls: 'here', r: 9 });
    } else {
      routes = line('route-bf-import', 'rt-import-casing') + line('route-bf-import', 'rt-import');
      if (mut.points.some((p) => p.path)) routes += line('route-bf-krone', 'rt-casing') + line('route-bf-krone', 'rt-line');
      if (a && a.kind === 'change' && a.type === 'visit') routes += line(a.ghost, 'rt-ghost');
    }
    if (map === 'bf' === (plan.map === 'bf')) for (const p of mut.points) {
      const pos = p.place ? DATA.place(p.place) : DATA.place(p.id); if (!pos) continue;
      marks += mk(pos, { cls: 'pt', r: 11, icon: { visit: 'i-flag', pass: 'i-ring', marker: 'i-drop' }[p.kind], label: p.kind === 'marker' ? p.name : p.name.split(',')[0] });
    }
    if (a) for (const m of Answers.marks(a, S)) {
      if ((m.pos.map === 'bf') !== (map === 'bf')) continue;
      const cls = m.kind === 'result' ? (m.sel ? 'res sel' : 'res') + (m.dim ? ' dimmed' : '') : m.kind;
      marks += mk(m.pos, { cls, r: m.sel || m.kind === 'place' || m.kind === 'newEnd' ? 15 : 12, ir: m.sel ? 9 : 7, icon: m.icon || (m.kind === 'dest' ? 'i-flag' : m.kind === 'oldEnd' || m.kind === 'newEnd' ? 'i-tent' : null), act: m.id ? 'selRow' : null, id: m.id, label: m.label });
    }
    MapView.setLayers(routes, marks);
  }

  // ---- the profile panel (website) ----
  const pct = (km, tot) => ((km / tot) * 100).toFixed(2) + '%';
  const band = (cls, ranges, tot) => ranges.map(([a, b]) => `<i class="${cls}" style="left:${pct(a, tot)};width:${pct(b - a, tot)}"></i>`).join('');
  function renderProfile() {
    const el = $('prof'); if (phone()) { el.innerHTML = ''; return; }
    const plan = S.plan(), mut = S.mut(), a = SHOW() ? S.ans : null, coll = S.profCollapsed;
    const collapse = `<button class="btn ghost sm icon" data-act="collapseProf" title="${coll ? 'Expand' : 'Collapse'}">${icon(coll ? 'i-chev-r' : 'i-chev-d')}</button>`;
    el.classList.toggle('collapsed', coll);
    if (plan.kind === 'trip') {
      const D = Answers.days(mut), tot = DATA.TRIP.km, d = S.selDay && D[S.selDay - 1];
      const head = d && !d.rest ? `<span class="h">Day ${d.n}${S.dates ? ' · ' + d.date : ''}</span><span class="cap">${esc(d.from)} → ${esc(d.to)} · ${d.km} km · ${num(d.climb)} m · ${hm(d.time)}</span>` : `<span class="h">Whole trip</span><span class="cap">${DATA.TRIP.facts}</span>`;
      let h = `<div class="ph">${head}<span class="right"><span class="cap">Whole trip · ${DATA.TRIP.facts}</span>${collapse}</span></div>`;
      if (!coll) {
        const riding = D.filter((x) => !x.rest), ends = riding.map((x) => x.start + x.km);
        const bands = riding.map((x) => S.selDay === x.n ? `<rect class="day-band sel" x="${x.start}" y="0" width="${x.km}" height="300"/>` : `<rect class="day-band col" style="fill:${dayTok(x.n)}" x="${x.start}" y="0" width="${x.km}" height="300"/>`).join('');
        const rules = ends.slice(0, -1).map((k) => `<line class="day-rule" x1="${k}" y1="0" x2="${k}" y2="300"/>`).join('');
        let ticks = '';
        if (a && a.kind === 'places') for (const r of a.rows) if (r.p.day) ticks += `<i class="tick ${S.selRow === r.p.id ? 'sel' : ''}" style="left:${pct(D[r.p.day - 1].start + r.p.km, tot)}" data-act="selRow" data-id="${esc(r.p.id)}"></i>`;
        const dends = riding.map((x) => `<i class="dend ${S.pointed && S.pointed.day === x.n ? 'pointed' : ''}" style="left:${pct(x.start + x.km, tot)}" data-act="pointEnd" data-n="${x.n}" title="End of Day ${x.n}"></i>`).join('');
        const prop = a && a.kind === 'change' && a.type === 'dayend' ? `<i class="dend old" style="left:${pct(D[3].start + mut.d4End, tot)}"></i><i class="dend new" style="left:${pct(D[3].start + a.km, tot)}"></i>` : '';
        const dayNums = D.map((x) => (x.rest ? `<b style="left:${pct(x.start, tot)};opacity:.6">rest</b>` : `<b class="${S.selDay === x.n ? 'sel' : ''}" style="left:${pct(x.start + x.km / 2, tot)};--dc:${dayTok(x.n)}">${x.n}</b>`)).join('');
        h += `<div class="pg"><span class="pl">3,000 m<br><span style="opacity:.7">0 m</span></span>
          <div class="strip" data-strip="${tot}"><svg class="prof" viewBox="0 0 ${tot} 300" preserveAspectRatio="none">${bands}<use href="#prof-alps-area" class="prof-area"/><use href="#prof-alps-line" class="prof-line"/><rect x="220" y="0" width="9" height="300" style="fill:var(--coral);opacity:.22"/>${rules}</svg>${ticks}${dends}${prop}</div>
          <span class="pl">Days</span><div class="days">${dayNums}</div>
          <span class="pl">Surface</span><div class="band"><i class="paved" style="left:0;width:100%"></i>${band('unknown', DATA.BANDS.unknown, tot)}${band('unpaved', DATA.BANDS.unpaved, tot)}</div>
          <span class="pl">Snow history</span><div class="band">${band('snow', DATA.BANDS.snow, tot)}</div>
          ${a && a.kind === 'gaps' && a.items.length ? `<span class="pl">Gaps</span><div class="band">${band('gap', a.items.filter((i) => i.band).map((i) => i.band), tot)}</div>` : ''}</div>`;
      }
      el.innerHTML = h; return;
    }
    let o = null, title = '';
    if (plan.kind === 'import') { o = DATA.ROUTES.import; title = `<span class="h">${o.title}</span><span class="cap">${o.facts}</span>`; }
    else if (a && a.kind === 'route' && !a.noData) { o = a.sel; title = `<span class="h">${a.data.title} · ${a.sel.name}</span><span class="cap">${o.km} km · ${num(o.climb)} m · ${hm(o.time)}</span>`; }
    else if (mut.route) { o = routeOption(mut.route); title = `<span class="h">${DATA.ROUTES[mut.route.id].title} · ${o.name}</span><span class="cap">${o.km} km · ${num(o.climb)} m · ${hm(o.time)}</span>`; }
    else title = `<span class="h">No route yet</span>`;
    let h = `<div class="ph">${title}<span class="right">${collapse}</span></div>`;
    if (!coll) {
      if (o && o.prof) h += `<div class="pg"><span class="pl">1,500 m<br><span style="opacity:.7">0 m</span></span><div class="strip"><svg class="prof" viewBox="0 150 ${o.profKm} 150" preserveAspectRatio="none"><use href="#${o.prof}-area" class="prof-area"/><use href="#${o.prof}-line" class="prof-line"/></svg></div>` +
        (o.surface ? `<span class="pl">Surface</span><div class="band"><i class="paved" style="left:0;width:100%"></i>${band('unpaved', o.surface.unpaved, o.profKm)}${band('unknown', o.surface.unknown, o.profKm)}</div>` : '') + `</div>`;
      else if (o) h += `<div class="note quiet">No profile for this option in the example data.</div>`;
    }
    el.innerHTML = h;
  }

  // ---- menus and popovers ----
  const mrow = (act, label, extra = '', data = '') => `<button class="mrow ${extra}" data-act="${act}" ${data}>${label}</button>`;
  function menuHtml(m) {
    const p = S.plan(), pr = S.preset();
    switch (m) {
      case 'plan': return `<div class="mt">Plans</div>` + DATA.PLANS.map((x) => mrow('switchPlan', esc(x.name), x.id === S.planId ? 'on' : '', `data-id="${x.id}"`)).join('');
      case 'dates': return `<div class="mt">Trip dates</div>${mrow('toggleDates', `${DATA.TRIP.dates}<span class="tg-r">${S.dates ? 'On' : 'Off'}</span>`, S.dates ? 'on' : '')}<div class="mc">${S.dates ? `Today: ${DATA.DAYS[S.today - 1].date} · Day ${S.today}` : 'Days have no dates; "open on the day" and "tomorrow" need one.'}</div>`;
      case 'proto': return `<div class="mt">Prototype · example data</div>${mrow('toggleFrame', S.framed ? 'Laptop view' : 'Phone view')}${mrow('reset', 'Reset')}`;
      case 'more': return `${mrow('menu', `Bike and goal<span class="tg-r">${DATA.BIKES[pr.bike]} · ${DATA.GOALS[pr.goal]}</span>`, '', 'data-menu="bike"')}${mrow('redo', 'Redo', S.future.length ? '' : 'dis')}${mrow('menu', 'Versions', '', 'data-menu="versions"')}${mrow('menu', 'Import', '', 'data-menu="import"')}${mrow('menu', 'Offline areas', '', 'data-menu="offline"')}${mrow('quiet', 'Export GPX', '', 'data-text="GPX export is not part of the prototype."')}${p.kind === 'trip' ? mrow('toggleDates', `Trip dates<span class="tg-r">${S.dates ? 'On' : 'Off'}</span>`) : ''}${S.framed ? mrow('toggleFrame', 'Laptop view') : ''}${mrow('reset', 'Reset')}<div class="mc">Prototype · example data</div>`;
      case 'versions': return `<div class="mt">Versions</div>${mrow('closeMenu', 'Now', 'on')}${mrow('closeMenu', 'Earlier today')}${mrow('closeMenu', 'Created')}<div class="mc">Versions are not part of the prototype.</div>`;
      case 'import': return `<div class="mt">Import a GPX file</div>${mrow('switchPlan', 'Schwarzwald Gravel, 3 days.gpx', '', 'data-id="import"')}<div class="mc">The line is kept as imported, not re-routed.</div>`;
      case 'offline': return `<div class="mt">Offline areas</div>${mrow('quiet', `Alpes du Sud · 212 MB<span class="tg-r">Download</span>`, '', 'data-text="Downloads are not part of the prototype."')}<div class="mc">Results come from downloaded areas.</div>`;
      case 'qr': return `<div class="mt">Continue on phone</div><div class="qr">${Array.from({ length: 81 }, (_, i) => `<i class="${(i * 7 + (i >> 3) * 3) % 5 < 2 ? 'on' : ''}"></i>`).join('')}</div><div class="mc">Scan with the OBC app. The plan opens there.</div>`;
      case 'bike': return `<div class="mt">Bike and goal</div>` + Box.pickerHtml('bike', S.req && S.req.kind === 'route' ? S.req : null, S.ans, S) + mrow('quiet', `${icon('i-sliders')}Advanced settings`, 'adv', 'data-text="Advanced settings are not part of the prototype."');
    }
    return '';
  }
  function renderMenus() {
    const layer = $('menus');
    if (!S.menu) { layer.innerHTML = ''; layer.hidden = true; return; }
    layer.hidden = false;
    layer.innerHTML = `<div class="scrim" data-act="closeMenu"></div><div class="menu menu-${S.menu}">${menuHtml(S.menu)}</div>`;
    if (!phone()) { const anchor = app().querySelector(`[data-menu="${S.menu}"]`), menu = layer.querySelector('.menu'); if (anchor) { const r = anchor.getBoundingClientRect(), ar = app().getBoundingClientRect(); menu.style.top = r.bottom - ar.top + 6 + 'px'; menu.style.left = Math.max(8, Math.min(r.left - ar.left, ar.width - menu.offsetWidth - 8)) + 'px'; } }
  }

  // ---- layout and wiring ----
  const mq = matchMedia('(min-width: 700px)');
  function layout() {
    S.layout = S.framed || !mq.matches ? 'phone' : 'web';
    render(); if (!phone()) app().style.height = ''; frameAll(true); render();
  }
  function onClick(e) {
    const t = e.target.closest('[data-act]'); if (!t || t.disabled || t.classList.contains('dis')) { if (!e.target.closest('#chips, .pick, .menu')) { if (S.picker) { S.picker = null; render(); } } return; }
    if (!(t.dataset.act in A)) return;
    if (t.dataset.act === 'pickField' && t.closest('.pick')) return;
    A[t.dataset.act](t.dataset);
    if (!['pickField', 'pickKind', 'pickWhere', 'pickWithin', 'pickOpen', 'pickBike', 'restore', 'toggleMore', 'menu', 'closeMenu', 'toggleDates'].includes(t.dataset.act) && !t.closest('.pick')) S.picker = null;
    if (!['menu', 'pickBike', 'toggleDates'].includes(t.dataset.act)) S.menu = null;
    render();
  }
  function init() {
    MapView.insets = () => S.insets();
    MapView.init($('map'), {
      tap: (d) => { if (d.act in A) { A[d.act](d); S.menu = null; render(); } },
      tapEmpty: () => { if (S.menu || S.picker) { S.menu = null; S.picker = null; render(); } },
      change: () => render(),
    });
    Sheet.init($('panel'), { onDetent: () => {} });
    Box.init({
      onInput: (v) => { S.text = v.slice(0, 80); S.edits = {}; S.removed = {}; S.edited = false; S.submitted = false; S.routeTo = null; S.picker = null; S.note = null; if (!S.text && !S.pointed) { clearBox(); render(); return; } run(); render(); if (phone() && S.ans && Sheet.detent === 'low') Sheet.setDetent('medium'); },
      onSubmit: () => { S.submitted = true; run(); render(); if (phone() && Sheet.detent === 'low') Sheet.setDetent('medium'); },
      onClear: () => { clearBox(); render(); },
      onFocus: () => { S.focused = true; Box.sync(S); if (S.menu) { S.menu = null; renderMenus(); } },
      onBlur: () => { S.focused = false; Box.sync(S); },
      onRemovePointed: () => { A.removePointed(); render(); },
    });
    app().addEventListener('click', onClick);
    $('prof').addEventListener('click', (e) => {
      const strip = e.target.closest('.strip'); if (!strip || e.target.closest('[data-act]') || S.plan().kind !== 'trip') return;
      const r = strip.getBoundingClientRect(), km = (e.clientX - r.left) / r.width * +strip.dataset.strip, D = Answers.days(S.mut()), d = D.find((x) => !x.rest && km >= x.start && km < x.start + x.km);
      if (d) { A.selDay({ n: d.n }); render(); }
    });
    document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && (S.menu || S.picker || S.pick)) { S.menu = null; S.picker = null; S.pick = false; render(); } if ((e.metaKey || e.ctrlKey) && e.key === 'z' && document.activeElement !== Box.input) { e.preventDefault(); e.shiftKey ? A.redo() : A.undo(); render(); } });
    mq.addEventListener('change', layout);
    if (window.visualViewport) visualViewport.addEventListener('resize', () => { if (phone() && !S.framed) { app().style.height = visualViewport.height + 'px'; scrollTo(0, 0); render(); } });
    document.addEventListener('gesturestart', (e) => e.preventDefault());
    layout();
    if (phone()) Sheet.setDetent('medium', false);
  }
  return { init, S, render, A };
})();
document.addEventListener('DOMContentLoaded', App.init);
