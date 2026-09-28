/* The chrome and the panel: the plans page, the tool bar, the nav bar, the sheet head, the chips,
   the body (days with their points, or an answer), the action row, the menus and the map callout.
   Pure functions of the state; the app calls them in one render pass. */

const View = (() => {
  const { esc, icon, num, hm, km1 } = Answers;
  const $ = (id) => document.getElementById(id);
  const btn = (act, label, ic, cls = 'ghost', extra = '') => `<button class="btn ${cls}" data-act="${act}" ${extra}>${ic ? icon(ic) : ''}${label ? `<span>${label}</span>` : ''}</button>`;
  const dayTok = (ci) => `var(--day-${(ci % 4) + 1})`;
  const kindIcon = (k) => (DATA.POINT_KINDS[k] || DATA.POINT_KINDS.pass).icon;

  // ---- the plans page: the app's list, with a sage track sketch per plan ----
  const BAND = `<svg viewBox="0 0 24 24"><rect x="2" y="7" width="17" height="10" rx="2"/><path d="M22 10v4"/><rect x="4.5" y="9.5" width="10" height="5" fill="currentColor" stroke="none"/></svg><span>82 %</span><svg viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7"/></svg><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3"/><path d="M12 2.5v3m0 13v3M2.5 12h3m13 0h3M5.3 5.3l2.1 2.1m9.2 9.2l2.1 2.1M5.3 18.7l2.1-2.1m9.2-9.2l2.1-2.1"/></svg>`;
  function sketch(p, S) {
    let pts, paths;
    if (p.kind === 'trip') { pts = Trip.line('alps').pts; paths = DATA.DAYS.filter((d) => !d.rest).map((d) => `#route-alps-d${d.n}`); }
    else if (p.kind === 'import') { pts = Line.sample('route-bf-import', 200); paths = ['#route-bf-import']; }
    else { const m = S.muts[p.id]; if (m && m.legs.length) { pts = Route.polyline(m).pts; paths = [Line.pathD(pts)]; } else { const h = DATA.place(p.here); pts = [[h.x, h.y]]; paths = []; } }
    const ref = (s, cls) => (s[0] === '#' ? `<use href="${s}" class="${cls}"/>` : `<path d="${s}" class="${cls}"/>`);
    const b = MapView.bboxOfPts(pts), pad = Math.max(b.w, b.h, 60) * 0.14, box = { x: b.x - pad, y: b.y - pad, w: b.w + 2 * pad, h: b.h + 2 * pad };
    const r = 3.5 * Math.max(box.w / 96, box.h / 72), a = pts[0], z = pts[pts.length - 1];
    return `<svg viewBox="${box.x} ${box.y} ${box.w} ${box.h}" preserveAspectRatio="xMidYMid meet">${paths.map((s) => ref(s, 'rt-casing')).join('')}${paths.map((s) => ref(s, 'rt-line')).join('')}<circle class="sk-a" cx="${a[0]}" cy="${a[1]}" r="${r}"/>${pts.length > 1 ? `<rect class="sk-b" x="${z[0] - r}" y="${z[1] - r}" width="${2 * r}" height="${2 * r}"/>` : ''}</svg>`;
  }
  function planLine(p, S) {
    if (p.kind === 'trip') return `${Trip.days(S.muts.alps || Trip.fresh()).length} days · ${DATA.TRIP.km} km · ${num(DATA.TRIP.climb)} m`;
    if (p.kind === 'import') { const r = DATA.ROUTES.import; return `Imported · ${r.km} km · ${num(r.climb)} m`; }
    const m = S.muts[p.id];
    if (m && m.legs.length) { const f = Route.figures(m); return `Route · ${km1(f.km)} km · ${num(f.climb)} m`; }
    return `Route · from ${DATA.place(p.here).name}`;
  }
  function plans(S) {
    const el = $('plans'); el.hidden = S.page !== 'plans';
    if (el.hidden) { el.innerHTML = ''; return; }
    const rows = DATA.PLANS.map((p) => `<button class="prow2" data-act="openPlan" data-id="${p.id}"><span class="sk">${sketch(p, S)}</span><span><span class="pn">${esc(p.name)}</span><span class="pm">${esc(planLine(p, S))}</span></span>${icon('i-chev-r', 'chev')}</button>`).join('');
    el.innerHTML = `<div class="band"><span>TRAILHEAD</span><span class="st">${BAND}</span></div><div class="wrap"><div class="tt"><h1>Routes and trips</h1><button class="new" data-act="openPlan" data-id="new" title="New route">${icon('i-plus')}<span>New route</span></button></div><div class="list">${rows}</div></div>`;
  }

  // ---- the tool bar (website) and the nav bar (phone) ----
  function bar(S) {
    const p = S.plan(), trip = p.kind === 'trip', pr = S.preset();
    $('bar').innerHTML = `<button class="back" data-act="plans">${icon('i-chev-l')}Routes and trips</button><span class="plan">${esc(p.name)}</span>
      ${trip ? `<button class="dates" data-act="menu" data-menu="dates">${icon('i-calendar')}${S.dates ? DATA.TRIP.datesShort : 'No dates'}</button>` : ''}
      <button class="preset" data-act="menu" data-menu="bike" title="Bike and goal">${icon('i-bike')}${DATA.BIKES[pr.bike]} · ${DATA.GOALS[pr.goal]}${icon('i-chev-d', 'chev')}</button>
      <button class="proto" data-act="menu" data-menu="proto">Prototype · example data</button>
      <div class="tools">${btn('undo', '', 'i-undo', 'ghost icon', `title="Undo" ${S.hist.length ? '' : 'disabled'}`)}${btn('redo', '', 'i-redo', 'ghost icon', `title="Redo" ${S.future.length ? '' : 'disabled'}`)}<span class="sep"></span>
      ${btn('menu', 'Versions', 'i-versions', 'ghost', 'data-menu="versions"')}${btn('menu', 'Import', 'i-import', 'ghost', 'data-menu="import"')}${btn('menu', 'Offline areas', 'i-download', 'ghost', 'data-menu="offline"')}${btn('menu', 'Continue on phone', 'i-qr', 'ghost', 'data-menu="qr"')}${btn('quiet', 'Export GPX', 'i-route', 'ghost', 'data-text="GPX export is not part of the prototype."')}<span class="sep"></span>
      ${btn('send', 'Send to device', 'i-send', 'primary')}</div>`;
  }
  function nav(S) {
    $('nav').innerHTML = `<button class="nb" data-act="plans" title="Routes and trips">${icon('i-chev-l')}</button><span class="title">${esc(S.plan().name)}</span>
      <span class="nr"><button class="nb ${S.hist.length ? '' : 'dis'}" data-act="undo" title="Undo">${icon('i-undo')}</button><button class="nb" data-act="menu" data-menu="more" title="More">${icon('i-more')}</button></span>`;
  }
  function chips(S, phone) {
    const c = $('chips');
    let h = '';
    if (S.req && (S.text || S.routeTo || S.edits.what)) h = Box.chipsHtml(Box.chips(S.req, S.ans, S), S) + Box.pickerHtml(S.picker, S.req, S.ans, S);
    else if (S.pointed) h = Box.findHtml() + (S.picker === 'where' ? Box.pickerHtml('where', null, null, S) : '');
    if (S.pick && !phone) h += `<div class="note quiet pickhint">Tap a day end on the map or the profile.<button class="btn sm" data-act="cancelPick">Cancel</button></div>`;
    if (c.innerHTML !== h) { c.innerHTML = h; Box.placeArrow(c); }
    c.hidden = !h;
  }
  // The rest line: the plan's totals.
  function restLine(S) {
    const p = S.plan(), m = S.mut(), pr = S.preset();
    if (p.kind === 'trip') { const D = Trip.days(m); return `<b>${DATA.TRIP.facts}</b><span class="cap">${D.length} days${S.dates ? ' · ' + DATA.TRIP.dates : ''} · ${DATA.BIKES[pr.bike]}</span>`; }
    if (p.kind === 'import') return `<b>${DATA.ROUTES.import.facts}</b><span class="cap">${DATA.ROUTES.import.notes[0]}</span>`;
    if (m.route) { const o = App.routeOption(m.route); return `<b>${DATA.ROUTES[m.route.id].title} · ${o.km} km</b><span class="cap">${num(o.climb)} m · ${hm(o.time)} · ${DATA.BIKES[pr.bike]}</span>`; }
    if (m.legs.length) { const f = Route.figures(m); return `<b>Route · ${km1(f.km)} km</b><span class="cap">${num(f.climb)} m · ${hm(f.time)} estimated · ${m.pts.length} points${f.days > 1 ? ` · ${f.days} days` : ''}</span>`; }
    return `<b>New route</b><span class="cap">From here · ${esc(DATA.place(S.here).name)} · ${DATA.BIKES[pr.bike]} · ${DATA.GOALS[pr.goal]}</span>`;
  }
  // The head: the totals at rest, the answer's count, the scrubber's reading, or the tool in use.
  function head(S, phone, show) {
    const el = $('head'), trip = S.plan().kind === 'trip', hd = show ? Answers.head(S.ans) : null;
    let txt, side = '', cls = 'rest';
    if (S.drawing) { cls = 'act'; txt = `<b>Draw the leg on the map.</b>`; side = btn('cancelDraw', 'Cancel', null, 'sm'); }
    else if (S.pick && phone) { cls = 'act'; txt = `<b>Tap a day end on the map.</b>`; side = btn('cancelPick', 'Cancel', null, 'sm'); }
    else if (show) { cls = 'ans'; txt = phone ? (hd ? `<b>${esc(hd.b)}</b><span class="cap">${esc(hd.span)}</span>` : '') : `${btn('clear', trip ? 'Back to the days' : 'Back', 'i-chev-l', 'ghost sm')}<span class="cap">${hd ? esc(hd.web || hd.b + (hd.span ? ' · ' + hd.span : '')) : ''}</span>`; }
    else if (phone && S.scrub != null && trip) { const r = Trip.reading(S.mut(), S.scrub); txt = `<b>${esc(r.b)}</b><span class="cap">${esc(r.span)}</span>`; }
    else txt = restLine(S);
    if (phone && !side) side = `<button class="sbtn" data-act="search" title="Search">${icon('i-search')}</button>`;
    el.innerHTML = phone ? `<div class="txt">${txt}</div>${side}` : txt + side;
    el.className = 'head ' + cls;
  }

  // ---- the days, each with its points on request ----
  function pointRows(S, pts, legs, dayKm0) {
    let h = '';
    pts.forEach((p, i) => {
      if (i) { const L = legs[i - 1]; h += `<button class="legrow" data-act="legMenu" data-key="${esc(L.key)}" data-leg="${L.i != null ? L.i : ''}">${icon(L.mode === 'straight' ? 'i-straight' : L.mode === 'drawn' ? 'i-pencil' : 'i-route')}${DATA.MODES[L.mode]}${icon('i-chev-d', 'chev')}</button>`; }
      const meta = [DATA.POINT_KINDS[p.kind] ? DATA.POINT_KINDS[p.kind].label : p.kind === 'start' ? 'Start' : p.kind, p.km != null ? `km ${Math.round(p.km - dayKm0)}` : null, p.elev != null ? `${num(p.elev)} m` : null, p.stop ? `${p.stop.via === 'outback' ? 'Out and back' : 'Through it'} · +${p.stop.km} km` : null].filter(Boolean).join(' · ');
      h += `<div class="row prow ptrow" data-act="pointCallout" data-pid="${esc(p.id)}"><span class="ic">${icon(p.kind === 'start' ? 'i-here' : kindIcon(p.kind))}</span><div><div class="t">${esc(p.name)}</div><div class="m">${esc(meta)}</div></div></div>`;
    });
    return h;
  }
  function daysHtml(S) {
    const mut = S.mut(), D = Trip.days(mut), max = Math.max(...D.map((d) => d.km));
    return D.map((d) => {
      const sel = S.selDay === d.n, open = S.expanded.has(d.n) && !d.rest, title = S.renaming === d.i ? `<input class="rename" value="${esc(d.title)}" data-i="${d.i}" aria-label="Day name">` : esc(d.title);
      let h = d.rest ? `<div class="day rest ${sel ? 'sel' : ''}" data-act="selDay" data-n="${d.n}" data-i="${d.i}"><b class="n rest">${d.n}</b><div class="t">${title}</div><span class="km">–</span><div class="m">${S.dates ? d.date : ''}</div>${sel ? `<button class="dmore" data-act="dayMenu" data-i="${d.i}" title="Day actions">${icon('i-more')}</button>` : ''}</div>`
        : `<div class="day ${sel ? 'sel' : ''} ${open ? 'open' : ''}" data-act="selDay" data-n="${d.n}" data-i="${d.i}" style="--dc:${dayTok(d.ci)}"><b class="n">${d.n}</b><div class="t">${title}</div><span class="km">${d.km} km</span><div class="m">${S.dates ? d.date + ' · ' : ''}${hm(d.time)} · ${num(d.climb)} m</div>${sel ? `<button class="dmore" data-act="dayMenu" data-i="${d.i}" title="Day actions">${icon('i-more')}</button>` : ''}<button class="dchev" data-act="expandDay" data-n="${d.n}" title="Points">${icon(open ? 'i-chev-d' : 'i-chev-r')}</button><i class="bar" style="width:calc((100% - 86px) * ${(d.km / max).toFixed(3)})"></i></div>`;
      if (open) h += `<div class="dpts">${pointRows(S, Trip.points(mut, d), Trip.legs(mut, d), d.start)}</div>`;
      return h;
    }).join('');
  }
  function routeHtml(S) {
    const p = S.plan(), m = S.mut(), f = Route.figures(m);
    let h = '';
    if (p.kind === 'import') { const r = DATA.ROUTES.import; h += `<dl class="ledger"><dt>Distance</dt><dd>${r.km} km</dd><dt>Climb</dt><dd>${num(r.climb)} m</dd><dt>Bike</dt><dd>Gravel</dd><dt>Surface</dt><dd>41 km <small>unknown</small></dd></dl>${r.notes.map((n) => `<div class="note quiet">${n}</div>`).join('')}`; }
    else if (m.route) { const o = App.routeOption(m.route); h += `<dl class="ledger"><dt>Route</dt><dd>${DATA.ROUTES[m.route.id].title}</dd><dt>Option</dt><dd>${o.name}</dd><dt>Distance</dt><dd>${o.km} km</dd><dt>Climb</dt><dd>${num(o.climb)} m</dd><dt>Riding time</dt><dd>${hm(o.time)} <small>estimated</small></dd></dl>`; }
    else if (m.legs.length) h += `<dl class="ledger"><dt>Distance</dt><dd>${km1(f.km)} km <small>estimated</small></dd><dt>Climb</dt><dd>${num(f.climb)} m <small>estimated</small></dd><dt>Riding time</dt><dd>${hm(f.time)} <small>estimated</small></dd>${f.days > 1 ? `<dt>Days</dt><dd>${f.days}</dd>` : ''}</dl>`;
    else h += `<dl class="ledger"><dt>Start</dt><dd>Here · ${esc(DATA.place(S.here).name)}</dd><dt>Bike</dt><dd>${DATA.BIKES[S.preset().bike]} · ${DATA.GOALS[S.preset().goal]}</dd></dl><div class="note quiet">Tap the map to add a point, or type a place.</div>`;
    const legs = m.legs.map((L, i) => ({ mode: L.mode, key: String(i), i, fixed: L.fixed }));
    h += `<div class="sec">Points</div>` + pointRows(S, m.pts, legs, 0);
    if (m.visits.length) h += `<div class="sec">Visits</div>` + m.visits.map((v) => `<div class="row prow"><span class="ic">${icon('i-flag')}</span><div><div class="t">${esc(v.name)}</div><div class="m">Visit${v.km ? ` · +${v.km} km` : ''}</div></div></div>`).join('');
    return h;
  }
  // The one list: the answer while one is open, else the days or the route's points.
  function body(S, phone, show) {
    const note = S.note ? `<div class="note">${esc(S.note.text)}${S.note.undo && S.hist.length ? `<button class="btn sm" data-act="undo">${icon('i-undo')}Undo</button>` : ''}</div>` : '';
    const b = $('body');
    if (show) { b.innerHTML = note + Answers.body(S.ans, S, phone); return; }
    b.innerHTML = note + (S.plan().kind === 'trip' ? daysHtml(S) : routeHtml(S));
    const ren = b.querySelector('input.rename'); if (ren) { ren.focus(); ren.select(); }
  }
  function pin(S, phone, show) {
    const acts = show && S.ans && !S.ans.noData && !S.focused ? S.ans.actions : null;
    $('pin').innerHTML = acts ? `${acts.secondary ? btn(acts.secondary.act, acts.secondary.label, null, 'ghost') : ''}${btn(acts.primary.act, acts.primary.label, acts.primary.icon, 'primary')}` : '';
    $('pin').hidden = !acts;
  }
  function foot(S) {
    const p = S.plan(), facts = p.kind === 'trip' ? `${DATA.TRIP.facts} · ${DATA.TRIP.unknown}` : p.kind === 'import' ? DATA.ROUTES.import.notes.slice(0, 2).join(' · ') : '';
    $('foot').innerHTML = facts ? `<span class="cap">${facts}</span>` : ''; $('foot').hidden = !facts;
  }

  // ---- menus ----
  const mrow = (act, label, extra = '', data = '') => `<button class="mrow ${extra}" data-act="${act}" ${data}>${label}</button>`;
  function menuHtml(S, m) {
    const p = S.plan(), pr = S.preset();
    switch (m) {
      case 'dates': return `<div class="mt">Trip dates</div>${mrow('toggleDates', `${DATA.TRIP.dates}<span class="tg-r">${S.dates ? 'On' : 'Off'}</span>`, S.dates ? 'on' : '')}<div class="mc">${S.dates ? `Today: ${Trip.days(S.mut())[S.today - 1].date} · Day ${S.today}` : 'Days have no dates; "open on the day" and "tomorrow" need one.'}</div>`;
      case 'proto': return `<div class="mt">Prototype · example data</div>${mrow('toggleFrame', S.framed ? 'Laptop view' : 'Phone view')}${S.framed ? mrow('rotate', S.rot ? 'Portrait' : 'Landscape') : ''}${mrow('reset', 'Reset')}`;
      case 'more': return `${mrow('send', 'Save and send to device')}${mrow('redo', 'Redo', S.future.length ? '' : 'dis')}${mrow('menu', 'Versions', '', 'data-menu="versions"')}${mrow('menu', 'Import', '', 'data-menu="import"')}${mrow('menu', 'Offline areas', '', 'data-menu="offline"')}${mrow('menu', 'Continue on phone', '', 'data-menu="qr"')}${mrow('quiet', 'Export GPX', '', 'data-text="GPX export is not part of the prototype."')}${mrow('menu', `Bike and goal<span class="tg-r">${DATA.BIKES[pr.bike]} · ${DATA.GOALS[pr.goal]}</span>`, '', 'data-menu="bike"')}${p.kind === 'trip' ? mrow('toggleDates', `Trip dates<span class="tg-r">${S.dates ? 'On' : 'Off'}</span>`) : ''}${mrow('menu', 'Prototype · example data', 'faint', 'data-menu="proto"')}`;
      case 'versions': return `<div class="mt">Versions</div>${mrow('closeMenu', 'Now', 'on')}${mrow('closeMenu', 'Earlier today')}${mrow('closeMenu', 'Created')}<div class="mc">Versions are not part of the prototype.</div>`;
      case 'import': return `<div class="mt">Import a GPX file</div>${mrow('openPlan', 'Schwarzwald Gravel, 3 days.gpx', '', 'data-id="import"')}<div class="mc">The line is kept as imported, not re-routed.</div>`;
      case 'offline': return `<div class="mt">Offline areas</div>${mrow('quiet', `Alpes du Sud · 212 MB<span class="tg-r">Download</span>`, '', 'data-text="Downloads are not part of the prototype."')}<div class="mc">Results come from downloaded areas.</div>`;
      case 'qr': return `<div class="mt">Continue on phone</div><div class="qr">${Array.from({ length: 81 }, (_, i) => `<i class="${(i * 7 + (i >> 3) * 3) % 5 < 2 ? 'on' : ''}"></i>`).join('')}</div><div class="mc">Scan with the OBC app. The plan opens there.</div>`;
      case 'bike': return `<div class="mt">Bike and goal</div>` + Box.pickerHtml('bike', S.req && S.req.kind === 'route' ? S.req : null, S.ans, S) + mrow('quiet', `${icon('i-sliders')}Advanced settings`, 'adv', 'data-text="Advanced settings are not part of the prototype."');
      case 'day': { const d = Trip.days(S.mut())[S.menuDay], last = S.menuDay >= S.mut().ends.length; return `<div class="mt">Day ${d.n}</div>${d.rest ? '' : mrow('dayEndAtStop', 'End at a stop', last ? 'dis' : '', `data-i="${d.i}"`)}${mrow('dayRename', 'Rename', '', `data-i="${d.i}"`)}${d.rest ? '' : mrow('daySplit', 'Split this day', '', `data-i="${d.i}"`)}${mrow('dayJoin', 'Join with the next day', last ? 'dis' : '', `data-i="${d.i}"`)}`; }
      case 'mode': return `<div class="mt">Leg</div>` + Object.entries(DATA.MODES).map(([k, l]) => mrow('setMode', l, S.menuMode === k ? 'on' : '', `data-mode="${k}"`)).join('');
    }
    return '';
  }
  function menus(S, phone) {
    const layer = $('menus');
    if (!S.menu) { layer.innerHTML = ''; layer.hidden = true; return; }
    layer.hidden = false;
    layer.innerHTML = `<div class="scrim" data-act="closeMenu"></div><div class="menu menu-${S.menu}">${menuHtml(S, S.menu)}</div>`;
    const menu = layer.querySelector('.menu'), ar = $('app').getBoundingClientRect();
    let r = S.menuAnchor; if (!r && !phone) { const a = $('app').querySelector(`[data-menu="${S.menu}"]`); r = a && a.getBoundingClientRect(); }
    if (r) {
      const below = r.bottom - ar.top + 6, fits = below + menu.offsetHeight < ar.height - 8;
      menu.style.top = (fits ? below : Math.max(8, r.top - ar.top - menu.offsetHeight - 6)) + 'px';
      menu.style.left = Math.max(8, Math.min(r.left - ar.left, ar.width - menu.offsetWidth - 8)) + 'px';
      menu.style.right = 'auto'; menu.classList.add('anchored');
    }
  }

  // ---- the callout on the map ----
  function calloutHtml(S, c) {
    const seg = (items, act, cur, data) => `<div class="co-seg">${items.map(([k, l, ic]) => `<button class="${cur === k ? 'on' : ''}" data-act="${act}" data-${data}="${k}">${ic ? icon(ic) : ''}${l}</button>`).join('')}</div>`;
    switch (c.kind) {
      case 'add': return `<div class="co-t">${esc(c.name)}</div>${c.meta ? `<div class="co-m">${esc(c.meta)}</div>` : ''}${btn('coAdd', 'Add point', 'i-plus', 'primary')}`;
      case 'place': { const p = c.place; return `<div class="co-t">${esc(p.name)}</div><div class="co-m">${esc(DATA.KIND_LINE[p.kind] || p.kind)} · ${km1(p.off || 0)} km off the line</div><div class="co-acts">${c.day ? btn('coEnd', `End Day ${c.day} here`, null, 'primary') : ''}${btn('coStop', 'Add as stop', null, '')}</div>`; }
      case 'offline': { const p = c.place; return `<div class="co-t">${esc(p.name)}</div><div class="co-m">${km1(p.off)} km off the line · how does Day ${c.day} reach it?</div>${[['outback', 'Out and back', `+${km1(p.off * 2)} km · +${p.climb || 0} m`], ['through', 'Through it', `+${km1(p.off * 1.2)} km · +${Math.round((p.climb || 0) / 2)} m`]].map(([v, l, m]) => `<button class="co-row" data-act="coVia" data-via="${v}"><b>${l}</b><span>${m}</span></button>`).join('')}`; }
      case 'point': { const p = c.point; return `<div class="co-t">${esc(p.name)}</div><div class="co-m">${esc(c.meta)}</div>${p.fixed || p.kind === 'start' ? '' : seg(Object.entries(DATA.POINT_KINDS).filter(([k]) => k !== 'marker').map(([k, v]) => [k, v.label, v.icon]), 'coKind', p.kind, 'kind')}${p.fixed || p.kind === 'start' ? '' : btn('coRemove', 'Remove point', 'i-close', '')}`; }
      case 'leg': return `<div class="co-t">${esc(c.title)}</div><div class="co-m">${esc(c.meta)}</div>${c.fixed ? `<div class="co-m">Imported line · not re-routed</div>` : seg([['routed', 'Routed', 'i-route'], ['straight', 'Straight', 'i-straight'], ['drawn', 'Drawn', 'i-pencil']], 'coMode', c.mode, 'mode')}`;
    }
    return '';
  }
  function callout(S) {
    const el = $('callout'), c = S.callout;
    if (!c) { el.hidden = true; el.innerHTML = ''; return; }
    el.hidden = false; el.innerHTML = `<div class="co">${calloutHtml(S, c)}<button class="co-x" data-act="coClose" title="Close">${icon('i-close')}</button></div><i class="co-arrow"></i>`;
    placeCallout(S);
  }
  function placeCallout(S) {
    const el = $('callout'), c = S.callout; if (!c || el.hidden) return;
    const p = MapView.toScreen(c.pos), map = $('map'), W = map.clientWidth, co = el.querySelector('.co'), w = co.offsetWidth, h = co.offsetHeight;
    const above = p.y - h - 18 > (S.insets().t || 0) + 8, x = Math.max(8, Math.min(W - w - 8, p.x - w / 2));
    el.style.left = x + 'px'; el.style.top = (above ? p.y - h - 16 : p.y + 20) + 'px';
    el.classList.toggle('below', !above);
    el.querySelector('.co-arrow').style.left = Math.max(12, Math.min(w - 12, p.x - x)) + 'px';
  }
  // Landscape: the profile alone, full width, with its header line.
  function land(S) {
    const p = S.plan(), f = p.kind === 'trip' ? `${DATA.TRIP.facts} · ${Trip.days(S.mut()).length} days` : p.kind === 'import' ? DATA.ROUTES.import.facts : restLine(S).replace(/<[^>]+>/g, ' ');
    const el = $('land'); el.innerHTML = `<div class="lh"><span>${esc(p.name)}</span><span class="cap">${esc(f)}</span></div><div class="slot"></div>`;
    el.querySelector('.slot').appendChild(App.pf);
  }
  return { plans, bar, nav, chips, head, body, pin, foot, menus, callout, placeCallout, land, dayTok, kindIcon, btn };
})();
