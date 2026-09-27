/* Answers: compute one answer from a request and the example data, and render it (the body, the
   one-line head, the action bar, the map marks). Fixed answer types: a list of places, one place,
   a route, a change to the route, a list of stretches (gaps), not understood. */

const Answers = (() => {
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const icon = (id, cls = '') => `<svg class="${cls}"><use href="#${id}"/></svg>`;
  const num = (n) => n.toLocaleString('en-GB');
  const hm = (min) => `${Math.floor(min / 60)} h ${String(Math.round(min % 60)).padStart(2, '0')}`;
  const km1 = (k) => (Math.round(k * 10) / 10).toLocaleString('en-GB');
  const RINGS = [2, 5, 10, 25];

  // ---- the Day 4 end model: distance and climb share, calibrated on the sheet's Saint-Michel figures ----
  let d4prof = null;
  function climbFrac(K) {
    if (!d4prof) {
      const d = document.getElementById('prof-d4-line').getAttribute('d').match(/-?\d+\.?\d*/g).map(Number), pts = [];
      for (let i = 0; i + 1 < d.length; i += 2) pts.push([d[i], (300 - d[i + 1]) * 10]);
      let c = 0; d4prof = pts.map(([km, e], i) => { if (i && e > pts[i - 1][1]) c += e - pts[i - 1][1]; return [km, c]; });
    }
    const tot = d4prof[d4prof.length - 1][1]; let c = 0;
    for (const [km, cc] of d4prof) { if (km > K) break; c = cc; }
    return c / tot;
  }
  function split(K) {
    const d4 = DATA.DAYS[3], d5 = DATA.DAYS[4], f = climbFrac(K);
    const t4 = Math.round(d4.time * (0.53 * K / d4.km + 0.47 * f) / 5) * 5, c4 = Math.round(d4.climb * f / 10) * 10;
    return { d4: { km: K, climb: c4, time: t4 }, d5: { km: d5.km + d4.km - K, climb: d5.climb + d4.climb - c4, time: d5.time + d4.time - t4 } };
  }
  const endName = (K) => (K >= 104 ? 'Valloire' : DATA.PLACES.filter((p) => p.day === 4 && p.km != null && !p.off && p.kind !== 'pass').reduce((a, p) => (Math.abs(p.km - K) < Math.abs(a.km - K) ? p : a)).name);
  // The trip's days with the Day 4 end applied: km, climb, time, from/to, start (trip km).
  function days(mut) {
    const K = mut.d4End, s = split(K); let start = 0;
    return DATA.DAYS.map((d) => {
      let x = { ...d };
      if (K !== 104 && d.n === 4) x = { ...x, ...s.d4, to: endName(K) };
      if (K !== 104 && d.n === 5) x = { ...x, ...s.d5, from: endName(K) };
      x.start = start; start += x.km; return x;
    });
  }
  const dayEndKm = (mut, n) => (n === 4 ? mut.d4End : DATA.DAYS[n - 1].km);
  const inRegion = (p, plan) => (plan.map === 'bf' ? p.map === 'bf' : p.map !== 'bf');

  // ---- where: label, ring default, the km window ----
  function whereInfo(w, S) {
    const mut = S.mut(), d = w.day && days(mut)[w.day - 1];
    switch (w.type) {
      case 'day': {
        const end = dayEndKm(mut, w.day);
        if (w.part === 'end') return { label: `End of Day ${w.day}`, k: d.to, ring: 5, text: `the end of Day ${w.day}` };
        if (w.part === 'start') return { label: `Start of Day ${w.day}`, k: d.from, ring: 5, text: `the start of Day ${w.day}` };
        if (w.part === 'middle') { const m = Math.round(end / 2); return { label: `Middle of Day ${w.day}`, k: `km ${m - 12}–${m + 12}`, ring: null, mid: m, text: `the middle of Day ${w.day}` }; }
        return { label: `Day ${w.day}`, k: null, ring: null, text: `Day ${w.day}` };
      }
      case 'route': return { label: 'Along the route', ring: 2, text: 'the route' };
      case 'view': return { label: 'In this map view', ring: null, text: 'this map view' };
      case 'here': return { label: 'Near here', k: DATA.place(S.here).name, ring: 5, text: 'here' };
      case 'near': return { label: `Near ${w.place.name}`, ring: 5, text: w.place.name };
      case 'needDate': return { label: 'Tomorrow', need: true, ring: null, text: 'tomorrow' };
    }
  }

  function places(req, S) {
    const plan = S.plan(), mut = S.mut(), w = req.where, wi = whereInfo(w, S), within = req.within || wi.ring;
    const cands = DATA.PLACES.filter((p) => req.kinds.includes(p.kind) && inRegion(p, plan));
    const D = days(mut), tripKm = (p) => (p.day ? D[p.day - 1].start + p.km : 0);
    let sections = [], note = null, sortText = '';
    const row = (p, meta) => ({ p, meta });
    if (w.type === 'day') {
      const end = dayEndKm(mut, w.day), onDay = cands.filter((p) => p.day === w.day && p.km <= end), later = cands.filter((p) => p.day === w.day && p.km > end);
      const byKm = (a, b) => a.km - b.km;
      if (w.part === 'end') {
        const near = onDay.filter((p) => end - p.km + p.off <= within).sort((a, b) => end - a.km + a.off - (end - b.km + b.off));
        const earlier = onDay.filter((p) => !near.includes(p)).sort((a, b) => b.km - a.km);
        sections.push({ title: `Within ${within} km of the end of Day ${w.day}`, rows: near.map((p) => row(p, `${p.km === end ? '' : `km ${p.km} · `}${km1(p.off)} km off the line${p.climb ? ` · +${p.climb} m` : ''}`)) });
        if (earlier.length) sections.push({ title: `Earlier on Day ${w.day}`, more: true, rows: earlier.map((p) => row(p, `km ${p.km} · ${km1(p.off)} km off the line`)) });
        sortText = 'nearest to the end first';
      } else if (w.part === 'start') {
        const near = onDay.filter((p) => p.km + p.off <= within).sort(byKm), rest = onDay.filter((p) => !near.includes(p)).sort(byKm);
        sections.push({ title: `Within ${within} km of the start of Day ${w.day}`, rows: near.map((p) => row(p, `km ${p.km} · ${km1(p.off)} km off the line`)) });
        if (rest.length) sections.push({ title: `Later on Day ${w.day}`, more: true, rows: rest.map((p) => row(p, `km ${p.km} · ${km1(p.off)} km off the line`)) });
        sortText = `by km on Day ${w.day}`;
      } else if (w.part === 'middle') {
        const mid = onDay.filter((p) => Math.abs(p.km - wi.mid) <= 12).sort(byKm), rest = onDay.filter((p) => !mid.includes(p)).sort(byKm);
        sections.push({ title: `Middle of Day ${w.day} · km ${wi.mid - 12}–${wi.mid + 12}`, rows: mid.map((p) => row(p, `km ${p.km} · ${km1(p.off)} km off the line`)) });
        if (rest.length) sections.push({ title: `Elsewhere on Day ${w.day}`, more: true, rows: rest.map((p) => row(p, `km ${p.km} · ${km1(p.off)} km off the line`)) });
        sortText = `by km on Day ${w.day}`;
      } else { sections.push({ rows: onDay.sort(byKm).map((p) => row(p, `km ${p.km} · ${km1(p.off)} km off the line`)) }); sortText = `by km on Day ${w.day}`; }
      if (later.length) sections.push({ title: `Later, on Day ${w.day + 1}`, more: true, rows: later.sort(byKm).map((p) => row(p, `km ${p.km - end} of Day ${w.day + 1} · ${km1(p.off)} km off the line`)) });
    } else if (w.type === 'route') {
      const rows = cands.filter((p) => p.day && p.off <= within).sort((a, b) => tripKm(a) - tripKm(b));
      sections.push({ rows: rows.map((p) => row(p, `Day ${p.day} · km ${p.km} · ${km1(p.off)} km off the line`)) }); sortText = 'by km along the route';
    } else if (w.type === 'here' || w.type === 'near') {
      const c = w.type === 'here' ? DATA.place(S.here) : w.place, dist = (p) => MapView.kmBetween(p.map === c.map ? p.map : 'alps', MapView.convert(p, p.map === c.map ? p.map : 'alps'), MapView.convert(c, p.map === c.map ? p.map : 'alps'));
      const rows = cands.filter((p) => dist(p) <= within).sort((a, b) => dist(a) - dist(b));
      sections.push({ rows: rows.map((p) => row(p, `${km1(dist(p))} km from ${w.type === 'here' ? 'here' : c.name}`)) }); sortText = 'nearest first';
    } else if (w.type === 'view') {
      const rect = MapView.viewRect(S.insets()), vis = cands.filter((p) => MapView.inRect(p, rect));
      let rows = vis, grown = null;
      if (!rows.length) for (const r of [2, 5, 8, 15, 25, 50]) { rows = cands.filter((p) => MapView.inRect(p, rect, r)); if (rows.length) { grown = r; break; } }
      if (grown) note = `None in this view. ${rows.length} found within ${grown} km.`;
      const onRoute = rows.some((p) => p.day);
      rows = onRoute ? rows.slice().sort((a, b) => tripKm(a) - tripKm(b)) : rows.slice().sort((a, b) => a.name.localeCompare(b.name));
      sections.push({ rows: rows.map((p) => row(p, p.day ? `Day ${p.day} · km ${p.km} · ${km1(p.off)} km off the line` : DATA.KIND_LINE[p.kind])) });
      sortText = onRoute ? 'by km along the route' : 'by name';
    } else if (w.type === 'needDate') { sections.push({ rows: [] }); }

    // the open-on filter
    let open = null, need = false;
    if (req.open) {
      let wd = req.open.wd;
      if (!wd && req.open.onDay) { if (S.dates) wd = w.day ? DATA.DAYS[w.day - 1].wd : 'any'; else need = true; }
      if (wd && wd !== 'any') {
        const wdl = DATA.WEEKDAYS[wd]; let openCount = 0, known = 0;
        for (const s of sections) {
          for (const r of s.rows) { const st = wd === 'sun' ? r.p.sun : undefined; r.state = st === true ? 'open' : st === false ? 'closed' : 'unknown'; if (st === true) openCount++; if (st !== undefined && st !== null) known++; }
          s.rows.sort((a, b) => (a.state === 'closed') - (b.state === 'closed'));
        }
        open = { wd, label: `Open on ${req.open.onDay && w.day ? DATA.DAYS[w.day - 1].date : wdl}`, sub: known ? `${openCount} open on ${wdl}` : `${wdl} hours unknown` };
      }
    }
    let rows = sections.flatMap((s) => s.rows);
    const ringed = sections[0] && sections[0].title && within != null, ring = ringed ? sections[0].rows : rows, n = ring.length, rest = rows.length - n;
    const kl = Parser.kindsLabel(req.kinds), kll = kl.toLowerCase();
    const one = req.kinds.length === 1 ? DATA.KINDS[req.kinds[0]] : null;
    const countLabel = n === 1 && one ? `1 ${one.one}` : `${n} ${one ? kll : 'places'}`;
    let empty = null;
    if (!n && w.type !== 'needDate') {
      rows = []; sections = [];
      const inRing = within != null;
      let nearest = null;
      if (w.type === 'day') { const end = dayEndKm(mut, w.day), c = cands.filter((p) => p.day).sort((a, b) => Math.abs(a.km - end) - Math.abs(b.km - end))[0]; if (c) nearest = row(c, `km ${c.km} of Day ${c.day} · ${Math.abs(end - c.km)} km ${c.km < end ? 'before' : 'after'} the end`); }
      else if (cands.length) { const c = cands[0]; nearest = row(c, c.day ? `Day ${c.day} · km ${c.km}` : DATA.KIND_LINE[c.kind]); }
      const wider = inRing && within < 25 ? 25 : null;
      empty = { text: inRing ? `No ${kll} within ${within} km of ${wi.text}.` : `No ${kll} in ${wi.text}.`, wider, nearest, none: !cands.length ? `No ${kll} in the example data.` : null };
    }
    const sleep = req.kinds.every((k) => DATA.SLEEP.includes(k)), endDay = plan.kind === 'trip' && w.type === 'day' && sleep;
    const actions = rows.length || (empty && empty.nearest) ? { primary: endDay ? { act: 'endDay', label: `End Day ${w.day} here` } : { act: 'addStop', label: 'Add as stop' }, secondary: endDay ? { act: 'addStop', label: 'Add as stop' } : null } : null;
    const head = { b: `${countLabel}${open ? ` · ${open.sub}` : ''}`, span: sortText };
    if (rest > 0 && sections[1]) head.web = `${n} within ${within} km · ${rest} ${sections[1].title.charAt(0).toLowerCase()}${sections[1].title.slice(1)}`;
    return { kind: 'places', kinds: req.kinds, where: w, wi, within, open, need, sections, rows, note, head, empty, actions, endDay: endDay ? w.day : null };
  }
  // Counts for the pickers: per kind and per ring, for the same where.
  const count = (req, S, patch) => { try { const a = places({ ...req, ...patch }, S); return a.sections.length && a.sections[0].title ? a.sections[0].rows.length : a.rows.length; } catch (e) { return 0; } };

  function route(req, S) {
    const to = req.to, data = Object.values(DATA.ROUTES).find((r) => r.to === to.id);
    if (!data) return { kind: 'route', to, noData: `No route data to ${to.name} in the example data.`, bike: req.bike, goal: req.goal };
    const sel = data.options.find((o) => o.id === S.selOpt) || data.options[0];
    return { kind: 'route', to, data, from: DATA.place(data.from), options: data.options, sel, bike: req.bike, goal: req.goal, single: data.options.length === 1,
      note: req.bike !== (data.bike || 'touring') && !data.bike ? 'Example data: the options do not change with the bike type.' : null,
      actions: { primary: { act: 'send', label: 'Send to device' } } };
  }

  function change(req, S) {
    const plan = S.plan(), mut = S.mut(), c = req.change;
    if (c.type === 'dayend') {
      const p = c.place;
      if (plan.kind !== 'trip') return { kind: 'change', noData: 'This plan has no days.' };
      if (c.day !== 4 || p.day !== 4 || p.km == null) return { kind: 'change', noData: `No km data for ${p.name} on Day ${c.day} in the example data.` };
      if (p.km === mut.d4End) return { kind: 'change', noData: `Day 4 already ends at ${p.name}.` };
      const D = days(mut), s = split(p.km), b4 = D[3], b5 = D[4];
      return { kind: 'change', type: 'dayend', day: 4, place: p, km: p.km,
        rows: [['Day 4', `${b4.km} km`, `${s.d4.km} km`, hm(b4.time), hm(s.d4.time)], ['Day 5', `${b5.km} km`, `${s.d5.km} km`, hm(b5.time), hm(s.d5.time)]],
        notes: [p.km < 99 && mut.d4End >= 99 ? 'Day 5 starts with the Col du Télégraphe.' : null, 'The line does not change.'].filter(Boolean),
        actions: { primary: { act: 'apply', label: 'Apply' }, secondary: { act: 'cancel', label: 'Cancel' } } };
    }
    const p = c.place;
    if (plan.kind !== 'import' || p.id !== DATA.ROUTES.import.krone.place) return { kind: 'change', noData: plan.map === 'bf' && p.map === 'bf' ? `No detour data for ${p.name} in the example data.` : `${p.name} is not near this route.` };
    if (mut.points.some((x) => x.id === p.id)) return { kind: 'change', noData: `${p.name} is already a visit on this route.` };
    const im = DATA.ROUTES.import, k = im.krone;
    return { kind: 'change', type: 'visit', place: p, ghost: k.path,
      rows: [['Trip', `${im.km} km`, `${km1(im.km + k.km)} km`, `${num(im.climb)} m`, `${num(im.climb + k.climb)} m`]],
      notes: [`${p.off} km off the line · +${k.km} km · +${k.climb} m`, 'Only the new detour is routed.', `${im.km} km of the line unchanged.`],
      actions: { primary: { act: 'apply', label: 'Apply' }, secondary: { act: 'cancel', label: 'Cancel' } } };
  }

  function gaps(req, S) {
    const plan = S.plan(), g = DATA.GAPS[req.gapKind];
    if (plan.kind === 'new' && !S.mut().route) return { kind: 'gaps', gapKind: req.gapKind, label: g.label, icon: g.icon, items: [], note: 'No route yet.' };
    const items = plan.kind === 'trip' ? g.items : req.gapKind === 'water' ? [{ title: 'No mapped water for 38 km', where: 'Day 2 · imported line', marker: null }] : [];
    return { kind: 'gaps', gapKind: req.gapKind, label: g.label, icon: g.icon, items, note: items.length ? null : `No stretch without a ${req.gapKind === 'water' ? 'mapped water point' : 'shop'} in the example data.`,
      actions: items.some((i) => i.start) ? { primary: { act: 'addMarker', label: 'Add marker' } } : null };
  }

  function compute(req, S) {
    switch (req.kind) {
      case 'places': return places(req, S);
      case 'place': return { kind: 'place', place: req.place, others: req.place.ambiguous ? [DATA.OTHER_KANDEL] : [], actions: { primary: { act: 'routeFrom', label: 'Route from here' }, secondary: { act: 'addToRoute', label: 'Add to route' } } };
      case 'route': return route(req, S);
      case 'change': return change(req, S);
      case 'gaps': return gaps(req, S);
      default: return { kind: 'none', place: req.place, partial: req.partial };
    }
  }

  // ---- render ----
  const hoursOf = (p) => p.hours || (p.kind === 'water' ? '' : '');
  function rowHtml(r, S, phone, act = 'selRow') {
    const p = r.p, sel = S.selRow === p.id, ic = DATA.KINDS[p.kind] ? DATA.KINDS[p.kind].icon : 'i-ring', hrs = hoursOf(p);
    const meta = phone ? [r.meta.replace(/ · \+\d+ m$/, ''), hrs].filter(Boolean).join(' · ') : r.meta;
    return `<div class="row prow ${sel ? 'sel' : ''} ${r.state === 'closed' ? 'dim' : ''}" data-act="${act}" data-id="${esc(p.id)}"><span class="ic">${icon(ic)}</span><div><div class="t">${esc(p.name)}</div><div class="mm"><span class="m">${esc(meta)}</span>${!phone && hrs ? `<span class="m hrs">${esc(hrs)}</span>` : ''}</div></div></div>`;
  }
  const placeLine = (p) => [DATA.KIND_LINE[p.kind] || p.kind, p.elev != null ? `${num(p.elev)} m` : null, p.region].filter(Boolean).join(' · ');
  const onRouteLine = (p) => (p.day && p.km != null ? `On the route: Day ${p.day} at km ${p.km}.` : '');

  function body(ans, S, phone) {
    if (!ans) return '';
    if (ans.noData) return `<div class="note">${esc(ans.noData)}</div>`;
    switch (ans.kind) {
      case 'places': {
        let h = '';
        if (ans.need) h += `<div class="note quiet">${icon('i-calendar')}Day ${ans.where.day || ''} has no date.<button class="btn sm" data-act="setDates">Set trip dates</button></div>`;
        if (ans.where.type === 'needDate') return h + `<div class="note quiet">${icon('i-calendar')}Tomorrow needs a date.<button class="btn sm" data-act="setDates">Set trip dates</button></div>`;
        if (ans.note) h += `<div class="note quiet">${esc(ans.note)}</div>`;
        if (ans.empty) {
          h += `<div class="note">${esc(ans.empty.text)}</div>`;
          if (ans.empty.wider) h += `<button class="btn" data-act="wider" data-km="${ans.empty.wider}">Search within ${ans.empty.wider} km</button>`;
          if (ans.empty.nearest) h += `<div class="sec">Nearest on the route</div>${rowHtml(ans.empty.nearest, S, phone)}`;
          else if (ans.empty.none) h += `<div class="note quiet">${esc(ans.empty.none)}</div>`;
          return h;
        }
        for (const s of ans.sections) {
          if (!s.rows.length) continue;
          if (s.more) {
            const open = S.moreOpen || !phone;
            h += phone ? `<div class="row prow more" data-act="toggleMore"><span class="ic">${icon(DATA.KINDS[ans.kinds[0]].icon)}</span><div><div class="t">${esc(s.title)}</div><div class="m">${s.rows.length} ${s.rows.length === 1 ? 'place' : 'places'} · km ${Math.min(...s.rows.map((r) => r.p.km))} to km ${Math.max(...s.rows.map((r) => r.p.km))}</div></div>${icon('i-chev-' + (open ? 'd' : 'r'), 'chev')}</div>` : `<div class="sec">${esc(s.title)}</div>`;
            if (open) h += s.rows.map((r) => rowHtml(r, S, phone)).join('');
          } else { if (s.title) h += `<div class="sec">${esc(s.title)}</div>`; h += s.rows.map((r) => rowHtml(r, S, phone)).join(''); }
        }
        return h;
      }
      case 'place': {
        const p = ans.place;
        return `<div class="place"><div class="n">${esc(p.name)}</div><div class="k">${esc(placeLine(p))}</div>${onRouteLine(p) ? `<div class="w">${esc(onRouteLine(p))}</div>` : ''}</div>` +
          (ans.others.length ? `<div class="sec">Other places with this name</div>${ans.others.map((o) => `<div class="row prow"><span class="ic">${icon('i-ring')}</span><div><div class="t">${esc(o.name)}</div><div class="m">${esc(o.line)}</div></div></div>`).join('')}` : '');
      }
      case 'route': {
        const goal = DATA.GOALS[ans.goal] || 'Balanced';
        let h = `<div class="rt-t"><span>${esc(ans.data.title)}</span><small>${esc(DATA.BIKES[ans.bike])} · ${esc(goal)}</small></div>`;
        if (ans.single) { const o = ans.sel; h += `<dl class="ledger">${o.ledger.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join('')}</dl><div class="note quiet">${esc(o.note)}</div>`; }
        else h += ans.options.map((o) => `<div class="row prow orow ${o.id === ans.sel.id ? 'sel' : ''}" data-act="selOpt" data-id="${o.id}"><span class="ic ${o.id === ans.sel.id ? 'rt' : ''}">${icon(o.id === ans.sel.id ? 'i-dot' : 'i-ring')}</span><div class="t">${esc(o.name)}</div><span class="win">${esc(o.win)}</span><div class="m">${esc(o.meta)}</div></div>`).join('');
        if (ans.note) h += `<div class="note quiet">${esc(ans.note)}</div>`;
        return h;
      }
      case 'change': {
        let h = `<div class="diff">${ans.rows.map((r) => `<span class="d">${r[0]}</span><span><span class="b">${r[1]}</span><span class="ar">→</span><span class="a">${r[2]}</span></span><span><span class="b">${r[3]}</span><span class="ar">→</span><span class="a">${r[4]}</span></span>`).join('')}</div>`;
        return h + ans.notes.map((n) => `<div class="note quiet">${esc(n)}</div>`).join('');
      }
      case 'gaps': {
        if (ans.note) return `<div class="note">${esc(ans.note)}</div>`;
        return ans.items.map((it, i) => `<div class="row prow ${S.selRow === 'gap' + i ? 'sel' : ''}" data-act="selRow" data-id="gap${i}"><span class="ic">${icon(ans.icon)}</span><div><div class="t">${esc(it.title)}</div><div class="m">${esc(it.where)}</div>${it.marker ? `<div class="m">${esc(it.marker)}</div>` : ''}</div></div>`).join('');
      }
      case 'none': {
        let h = `<div class="note quiet">${ans.partial ? 'Not understood as one request.' : 'Not understood as a request.'} Place search:</div>`;
        if (ans.place) { const p = ans.place; h += `<div class="place"><div class="n">${esc(p.name)}</div><div class="k">${esc(placeLine(p))}</div>${onRouteLine(p) ? `<div class="w">${esc(onRouteLine(p))}</div>` : ''}</div>`; }
        else h += `<div class="note quiet">No place with this name.</div>`;
        return h;
      }
    }
    return '';
  }
  function head(ans) {
    if (!ans || ans.noData) return null;
    if (ans.kind === 'places') return ans.empty ? { b: '0 found', span: '' } : ans.head;
    if (ans.kind === 'place') return { b: ans.place.name, span: placeLine(ans.place) };
    if (ans.kind === 'route') return { b: ans.single ? `${ans.sel.km} km · ${num(ans.sel.climb)} m` : `${ans.options.length} options`, span: ans.single ? hm(ans.sel.time) : 'named by what they win' };
    if (ans.kind === 'change') return { b: ans.type === 'dayend' ? `End Day ${ans.day} at ${ans.place.name}` : `Add a visit · ${ans.place.name}`, span: 'proposal' };
    if (ans.kind === 'gaps') return { b: `${ans.items.length} ${ans.items.length === 1 ? 'stretch' : 'stretches'}`, span: ans.items.length ? 'longest first' : '' };
    if (ans.kind === 'none') return { b: 'Not understood', span: '' };
    return null;
  }

  // ---- map marks for an answer (as {pos, ...} specs; the app draws them) ----
  function marks(ans, S) {
    const out = [];
    if (!ans || ans.noData) return out;
    if (ans.kind === 'places') {
      const rows = ans.rows.length ? ans.rows : ans.empty && ans.empty.nearest ? [ans.empty.nearest] : [];
      for (const r of rows) out.push({ pos: r.p, kind: 'result', id: r.p.id, icon: DATA.KINDS[r.p.kind].icon, sel: S.selRow === r.p.id, label: S.selRow === r.p.id ? r.p.name.split(',')[0] : null, dim: r.state === 'closed' });
    } else if (ans.kind === 'place' || ans.kind === 'none') {
      if (ans.place) out.push({ pos: ans.place, kind: 'place', icon: ans.place.kind === 'summit' || ans.place.kind === 'pass' ? 'i-mountain' : 'i-flag', label: ans.place.name + (ans.place.elev ? ` · ${num(ans.place.elev)} m` : '') });
    } else if (ans.kind === 'route') {
      out.push({ pos: ans.to, kind: 'dest', label: ans.to.name });
    } else if (ans.kind === 'change' && ans.type === 'dayend') {
      out.push({ pos: MapView.d4Point(S.mut().d4End), kind: 'oldEnd', label: `${endName(S.mut().d4End)} · now` });
      out.push({ pos: MapView.d4Point(ans.km), kind: 'newEnd', label: `${ans.place.name} · proposed` });
    } else if (ans.kind === 'change' && ans.type === 'visit') {
      out.push({ pos: ans.place, kind: 'place', icon: 'i-flag', label: `${ans.place.name} · proposed` });
    } else if (ans.kind === 'gaps') {
      ans.items.forEach((it, i) => { if (it.start) out.push({ pos: DATA.place(it.start), kind: 'gap', icon: ans.icon, label: it.title, sel: S.selRow === 'gap' + i }); });
      if (ans.gapKind === 'water') for (const p of DATA.PLACES.filter((p) => p.kind === 'water')) out.push({ pos: p, kind: 'result', icon: 'i-drop', id: p.id, label: p.name });
    }
    return out;
  }

  return { compute, count, body, head, marks, days, split, endName, dayEndKm, whereInfo, esc, icon, num, hm, km1, RINGS };
})();
