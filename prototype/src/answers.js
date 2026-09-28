/* Answers: compute one answer from a request and the example data, and render it (the body, the
   one-line head, the action row, the map marks). Fixed answer types: a list of places, one place,
   a route, a change to the route, a list of stretches (gaps), not understood. */

const Answers = (() => {
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const icon = (id, cls = '') => `<svg class="${cls}"><use href="#${id}"/></svg>`;
  const num = (n) => n.toLocaleString('en-GB');
  const hm = (min) => `${Math.floor(min / 60)} h ${String(Math.round(min % 60)).padStart(2, '0')}`;
  const km1 = (k) => (Math.round(k * 10) / 10).toLocaleString('en-GB');
  const RINGS = [2, 5, 10, 25];
  const inRegion = (p, plan) => (plan.map === 'bf' ? p.map === 'bf' : p.map !== 'bf');
  const onLine = (p) => p.day && p.km != null;

  // ---- where: label, ring default, the km window ----
  function whereInfo(w, S) {
    const D = w.day && Trip.days(S.mut()), d = D && D[w.day - 1];
    switch (w.type) {
      case 'day': {
        if (!d || d.rest) return { label: `Day ${w.day}`, k: null, ring: null, text: `Day ${w.day}` };
        if (w.part === 'end') return { label: `End of Day ${w.day}`, k: d.to, ring: 5, text: `the end of Day ${w.day}` };
        if (w.part === 'start') return { label: `Start of Day ${w.day}`, k: d.from, ring: 5, text: `the start of Day ${w.day}` };
        if (w.part === 'middle') { const m = Math.round(d.km / 2); return { label: `Middle of Day ${w.day}`, k: `km ${m - 12}–${m + 12}`, ring: null, mid: m, text: `the middle of Day ${w.day}` }; }
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
    const D = plan.kind === 'trip' ? Trip.days(mut) : [], tk = (p) => (onLine(p) ? Trip.tripKm(p) : 0), dayOf = (p) => Trip.dayAt(D, tk(p));
    const onDayMeta = (p, d) => `km ${Math.round(tk(p) - d.start)} · ${km1(p.off)} km off the line`;
    let sections = [], note = null, sortText = '';
    const row = (p, meta) => ({ p, meta });
    if (w.type === 'day' && D[w.day - 1] && !D[w.day - 1].rest) {
      const d = D[w.day - 1], next = D.slice(w.day).find((x) => !x.rest), onDay = cands.filter((p) => onLine(p) && tk(p) >= d.start && tk(p) <= d.end), later = next ? cands.filter((p) => onLine(p) && tk(p) > d.end && tk(p) <= next.end) : [];
      const byKm = (a, b) => tk(a) - tk(b), gap = (p) => d.end - tk(p) + p.off;
      if (w.part === 'end') {
        const near = onDay.filter((p) => gap(p) <= within).sort((a, b) => gap(a) - gap(b)), earlier = onDay.filter((p) => !near.includes(p)).sort((a, b) => tk(b) - tk(a));
        sections.push({ title: `Within ${within} km of the end of Day ${w.day}`, rows: near.map((p) => row(p, `${tk(p) === d.end ? '' : `km ${Math.round(tk(p) - d.start)} · `}${km1(p.off)} km off the line${p.climb ? ` · +${p.climb} m` : ''}`)) });
        if (earlier.length) sections.push({ title: `Earlier on Day ${w.day}`, more: true, rows: earlier.map((p) => row(p, onDayMeta(p, d))) });
        sortText = 'nearest to the end first';
      } else if (w.part === 'start') {
        const near = onDay.filter((p) => tk(p) - d.start + p.off <= within).sort(byKm), rest = onDay.filter((p) => !near.includes(p)).sort(byKm);
        sections.push({ title: `Within ${within} km of the start of Day ${w.day}`, rows: near.map((p) => row(p, onDayMeta(p, d))) });
        if (rest.length) sections.push({ title: `Later on Day ${w.day}`, more: true, rows: rest.map((p) => row(p, onDayMeta(p, d))) });
        sortText = `by km on Day ${w.day}`;
      } else if (w.part === 'middle') {
        const mid = onDay.filter((p) => Math.abs(tk(p) - d.start - wi.mid) <= 12).sort(byKm), rest = onDay.filter((p) => !mid.includes(p)).sort(byKm);
        sections.push({ title: `Middle of Day ${w.day} · km ${wi.mid - 12}–${wi.mid + 12}`, rows: mid.map((p) => row(p, onDayMeta(p, d))) });
        if (rest.length) sections.push({ title: `Elsewhere on Day ${w.day}`, more: true, rows: rest.map((p) => row(p, onDayMeta(p, d))) });
        sortText = `by km on Day ${w.day}`;
      } else { sections.push({ rows: onDay.sort(byKm).map((p) => row(p, onDayMeta(p, d))) }); sortText = `by km on Day ${w.day}`; }
      if (later.length) sections.push({ title: `Later, on Day ${next.n}`, more: true, rows: later.sort(byKm).map((p) => row(p, `km ${Math.round(tk(p) - next.start)} of Day ${next.n} · ${km1(p.off)} km off the line`)) });
    } else if (w.type === 'day') { sections.push({ rows: [] }); }
    else if (w.type === 'route') {
      const rows = cands.filter((p) => onLine(p) && p.off <= within).sort((a, b) => tk(a) - tk(b));
      sections.push({ rows: rows.map((p) => { const d = dayOf(p); return row(p, `Day ${d.n} · ${onDayMeta(p, d)}`); }) }); sortText = 'by km along the route';
    } else if (w.type === 'here' || w.type === 'near') {
      const c = w.type === 'here' ? DATA.place(S.here) : w.place, dist = (p) => MapView.kmBetween(p.map === c.map ? p.map : 'alps', MapView.convert(p, p.map === c.map ? p.map : 'alps'), MapView.convert(c, p.map === c.map ? p.map : 'alps'));
      const rows = cands.filter((p) => dist(p) <= within).sort((a, b) => dist(a) - dist(b));
      sections.push({ rows: rows.map((p) => row(p, `${km1(dist(p))} km from ${w.type === 'here' ? 'here' : c.name}`)) }); sortText = 'nearest first';
    } else if (w.type === 'view') {
      const rect = MapView.viewRect(S.insets()), vis = cands.filter((p) => MapView.inRect(p, rect));
      let rows = vis, grown = null;
      if (!rows.length) for (const r of [2, 5, 8, 15, 25, 50]) { rows = cands.filter((p) => MapView.inRect(p, rect, r)); if (rows.length) { grown = r; break; } }
      if (grown) note = `None in this view. ${rows.length} found within ${grown} km.`;
      const onRoute = rows.some(onLine);
      rows = onRoute ? rows.slice().sort((a, b) => tk(a) - tk(b)) : rows.slice().sort((a, b) => a.name.localeCompare(b.name));
      sections.push({ rows: rows.map((p) => row(p, onLine(p) && D.length ? `Day ${dayOf(p).n} · ${onDayMeta(p, dayOf(p))}` : DATA.KIND_LINE[p.kind])) });
      sortText = onRoute ? 'by km along the route' : 'by name';
    } else if (w.type === 'needDate') { sections.push({ rows: [] }); }

    // the open-on filter
    let open = null, need = false;
    if (req.open) {
      let wd = req.open.wd;
      if (!wd && req.open.onDay) { if (S.dates) wd = w.day && D[w.day - 1] ? D[w.day - 1].wd : 'any'; else need = true; }
      if (wd && wd !== 'any') {
        const wdl = DATA.WEEKDAYS[wd]; let openCount = 0, known = 0;
        for (const s of sections) {
          for (const r of s.rows) { const st = wd === 'sun' ? r.p.sun : undefined; r.state = st === true ? 'open' : st === false ? 'closed' : 'unknown'; if (st === true) openCount++; if (st !== undefined && st !== null) known++; }
          s.rows.sort((a, b) => (a.state === 'closed') - (b.state === 'closed'));
        }
        open = { wd, label: `Open on ${req.open.onDay && w.day && D[w.day - 1] ? D[w.day - 1].date : wdl}`, sub: known ? `${openCount} open on ${wdl}` : `${wdl} hours unknown` };
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
      if (w.type === 'day' && D[w.day - 1]) { const d = D[w.day - 1], c = cands.filter(onLine).sort((a, b) => Math.abs(tk(a) - d.end) - Math.abs(tk(b) - d.end))[0]; if (c) { const cd = dayOf(c); nearest = row(c, `km ${Math.round(tk(c) - cd.start)} of Day ${cd.n} · ${Math.round(Math.abs(d.end - tk(c)))} km ${tk(c) < d.end ? 'before' : 'after'} the end`); } }
      else if (cands.length) { const c = cands[0]; nearest = row(c, onLine(c) && D.length ? `Day ${dayOf(c).n} · km ${Math.round(tk(c) - dayOf(c).start)}` : DATA.KIND_LINE[c.kind]); }
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
    const to = req.to, id = Object.keys(DATA.ROUTES).find((k) => DATA.ROUTES[k].to === to.id), data = id && DATA.ROUTES[id];
    if (!data) return { kind: 'route', to, noData: `No route data to ${to.name} in the example data.`, bike: req.bike, goal: req.goal };
    const sel = data.options.find((o) => o.id === S.selOpt) || data.options[0];
    return { kind: 'route', to, id, data, from: DATA.place(data.from), options: data.options, sel, bike: req.bike, goal: req.goal, single: data.options.length === 1,
      note: req.bike !== (data.bike || 'touring') && !data.bike ? 'Example data: the options do not change with the bike type.' : null,
      actions: { primary: { act: 'useRoute', label: 'Use this route' } } };
  }

  // A proposal to end a day at a place: the two days before → after; the line does not change.
  function dayEndChange(S, n, p) {
    const mut = S.mut(), D = Trip.days(mut), d = D[n - 1];
    if (!d || d.rest) return { kind: 'change', noData: `No Day ${n} in this plan.` };
    if (!onLine(p)) return { kind: 'change', noData: `No km data for ${p.name} in the example data.` };
    const km = Trip.tripKm(p), next = D.slice(n).find((x) => !x.rest);
    if (Math.abs(km - d.end) < 0.5) return { kind: 'change', noData: `Day ${n} already ends at ${p.name}.` };
    if (km < d.start + Trip.MIN_DAY || !next || km > next.end - Trip.MIN_DAY) return { kind: 'change', noData: `${p.name} is not within Day ${n} or Day ${n + 1}.` };
    const m2 = JSON.parse(JSON.stringify(mut)); Trip.setEnd(m2, d.i, km); const D2 = Trip.days(m2), a4 = D2[d.i], a5 = D2[next.i];
    const pass = DATA.PLACES.find((x) => x.kind === 'pass' && onLine(x) && Trip.tripKm(x) > Math.min(km, d.end) && Trip.tripKm(x) < Math.max(km, d.end));
    return { kind: 'change', type: 'dayend', day: n, i: d.i, place: p, km,
      rows: [[`Day ${d.n}`, `${d.km} km`, `${a4.km} km`, hm(d.time), hm(a4.time)], [`Day ${next.n}`, `${next.km} km`, `${a5.km} km`, hm(next.time), hm(a5.time)]],
      notes: [pass ? `Day ${km < d.end ? next.n : d.n} ${km < d.end ? 'starts' : 'ends'} with the ${pass.name}.` : null, 'The line does not change.'].filter(Boolean),
      actions: { primary: { act: 'apply', label: 'Apply' }, secondary: { act: 'cancel', label: 'Cancel' } } };
  }
  function change(req, S) {
    const plan = S.plan(), mut = S.mut(), c = req.change, p = c.place;
    if (c.type === 'dayend') return plan.kind !== 'trip' ? { kind: 'change', noData: 'This plan has no days.' } : dayEndChange(S, c.day, p);
    if (plan.kind !== 'import' || p.id !== DATA.ROUTES.import.krone.place) return { kind: 'change', noData: plan.map === 'bf' && p.map === 'bf' ? `No detour data for ${p.name} in the example data.` : `${p.name} is not near this route.` };
    if (mut.visits.some((x) => x.id === p.id)) return { kind: 'change', noData: `${p.name} is already a visit on this route.` };
    const im = DATA.ROUTES.import, k = im.krone;
    return { kind: 'change', type: 'visit', place: p, ghost: k.path,
      rows: [['Trip', `${im.km} km`, `${km1(im.km + k.km)} km`, `${num(im.climb)} m`, `${num(im.climb + k.climb)} m`]],
      notes: [`${p.off} km off the line · +${k.km} km · +${k.climb} m`, 'Only the new detour is routed.', `${im.km} km of the line unchanged.`],
      actions: { primary: { act: 'apply', label: 'Apply' }, secondary: { act: 'cancel', label: 'Cancel' } } };
  }

  function gaps(req, S) {
    const plan = S.plan(), g = DATA.GAPS[req.gapKind];
    if (plan.kind === 'new' && S.mut().legs.length === 0) return { kind: 'gaps', gapKind: req.gapKind, label: g.label, icon: g.icon, items: [], note: 'No route yet.' };
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
  function rowHtml(r, S, phone, act = 'selRow') {
    const p = r.p, sel = S.selRow === p.id, ic = DATA.KINDS[p.kind] ? DATA.KINDS[p.kind].icon : 'i-ring', hrs = p.hours || '';
    const meta = phone ? [r.meta.replace(/ · \+\d+ m$/, ''), hrs].filter(Boolean).join(' · ') : r.meta;
    return `<div class="row prow ${sel ? 'sel' : ''} ${r.state === 'closed' ? 'dim' : ''}" data-act="${act}" data-id="${esc(p.id)}"><span class="ic">${icon(ic)}</span><div><div class="t">${esc(p.name)}</div><div class="mm"><span class="m">${esc(meta)}</span>${!phone && hrs ? `<span class="m hrs">${esc(hrs)}</span>` : ''}</div></div></div>`;
  }
  const placeLine = (p) => [DATA.KIND_LINE[p.kind] || p.kind, p.elev != null ? `${num(p.elev)} m` : null, p.region].filter(Boolean).join(' · ');
  const onRouteLine = (p, S) => { if (!onLine(p) || S.plan().kind !== 'trip') return ''; const d = Trip.dayAt(Trip.days(S.mut()), Trip.tripKm(p)); return `On the route: Day ${d.n} at km ${Math.round(Trip.tripKm(p) - d.start)}.`; };

  function body(ans, S, phone) {
    if (!ans) return '';
    if (ans.noData) return `<div class="note">${esc(ans.noData)}</div>`;
    switch (ans.kind) {
      case 'places': {
        let h = '';
        if (ans.need) h += `<div class="note quiet">${icon('i-calendar')}Day ${ans.where.day || ''} has no date.<button class="btn sm" data-act="setDates">Set trip dates</button></div>`;
        if (ans.where.type === 'needDate') return h + `<div class="note quiet">${icon('i-calendar')}Tomorrow needs a date.<button class="btn sm" data-act="setDates">Set trip dates</button></div>`;
        if (ans.note && !phone) h += `<div class="note quiet">${esc(ans.note)}</div>`;
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
            h += phone ? `<div class="row prow more" data-act="toggleMore"><span class="ic">${icon(DATA.KINDS[ans.kinds[0]].icon)}</span><div><div class="t">${esc(s.title)}</div><div class="m">${s.rows.length} ${s.rows.length === 1 ? 'place' : 'places'}</div></div>${icon('i-chev-' + (open ? 'd' : 'r'), 'chev')}</div>` : `<div class="sec">${esc(s.title)}</div>`;
            if (open) h += s.rows.map((r) => rowHtml(r, S, phone)).join('');
          } else { if (s.title) h += `<div class="sec">${esc(s.title)}</div>`; h += s.rows.map((r) => rowHtml(r, S, phone)).join(''); }
        }
        return h;
      }
      case 'place': {
        const p = ans.place, w = onRouteLine(p, S);
        return `<div class="place"><div class="n">${esc(p.name)}</div><div class="k">${esc(placeLine(p))}</div>${w ? `<div class="w">${esc(w)}</div>` : ''}</div>` +
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
        if (ans.place) { const p = ans.place, w = onRouteLine(p, S); h += `<div class="place"><div class="n">${esc(p.name)}</div><div class="k">${esc(placeLine(p))}</div>${w ? `<div class="w">${esc(w)}</div>` : ''}</div>`; }
        else h += `<div class="note quiet">No place with this name.</div>`;
        return h;
      }
    }
    return '';
  }
  function head(ans) {
    if (!ans || ans.noData) return null;
    if (ans.kind === 'places') return ans.empty ? { b: '0 found', span: ans.empty.text } : ans.note ? { b: 'None in this view', span: ans.note.replace('None in this view. ', '') } : ans.head;
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
      const D = Trip.days(S.mut()), map = MapView.view.map;
      out.push({ pos: Trip.pointAt(D[ans.i].end, map), kind: 'oldEnd', label: `${D[ans.i].to} · now` });
      out.push({ pos: Trip.pointAt(ans.km, map), kind: 'newEnd', label: `${ans.place.name.split(',')[0]} · proposed` });
    } else if (ans.kind === 'change' && ans.type === 'visit') {
      out.push({ pos: ans.place, kind: 'place', icon: 'i-flag', label: `${ans.place.name} · proposed` });
    } else if (ans.kind === 'gaps') {
      ans.items.forEach((it, i) => { if (it.start) out.push({ pos: DATA.place(it.start), kind: 'gap', icon: ans.icon, label: it.title, sel: S.selRow === 'gap' + i }); });
      if (ans.gapKind === 'water') for (const p of DATA.PLACES.filter((p) => p.kind === 'water')) out.push({ pos: p, kind: 'result', icon: 'i-drop', id: p.id, label: p.name });
    }
    return out;
  }

  return { compute, count, body, head, marks, dayEndChange, whereInfo, onLine, esc, icon, num, hm, km1, RINGS };
})();
