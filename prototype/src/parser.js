/* The fake parser: a keyword matcher over one table of words. It stands in for the model (#2238)
   and is deliberately not clever. Parser.parse(text, ctx) → request. */

const Parser = (() => {
  // [phrases, field, value]. Phrases are separated by |, tokens by a space; N matches a number.
  const WORDS = [
    ['campsite|campsites|camping|campground|campgrounds|zeltplatz|zeltplatze|campingplatz|campingplatze|tent|tents', 'kind', 'campsite'],
    ['hotel|hotels|lodging|hostel|hostels|guesthouse|pension|unterkunft|unterkunfte|bnb|b&b|inn', 'kind', 'lodging'],
    ['places to sleep|place to sleep|sleep|somewhere to sleep|schlafen|ubernachten|ubernachtung|accommodation', 'kind', 'sleep'],
    ['shop|shops|supermarket|supermarkets|supermarkt|supermarkte|grocery|groceries|store|stores|laden|einkaufen|resupply|food|bakery|backer|backerei', 'kind', 'shop'],
    ['pharmacy|pharmacies|apotheke|apotheken|chemist|chemists', 'kind', 'pharmacy'],
    ['bike shop|bike shops|bikeshop|bikeshops|bike repair|bicycle shop|bicycle shops|fahrradladen|fahrradwerkstatt|radladen|bike mechanic', 'kind', 'bikeshop'],
    ['water|fountain|fountains|wasser|brunnen|trinkwasser|drinking water|tap', 'kind', 'water'],
    ['train|trains|train station|station|bahnhof|railway|zug', 'kind', 'train'],
    ['hut|huts|hutte|hutten|refuge|refuges', 'kind', 'hut'],
    ['shelter|shelters|schutzhutte', 'kind', 'shelter'],
    ['end of|end|ende|finish|ziel', 'part', 'end'],
    ['middle of|middle|mid|halfway|mitte|half', 'part', 'middle'],
    ['start of|start|beginning of|beginning|anfang|begin', 'part', 'start'],
    ['whole day|all day|ganzer tag|den ganzen tag', 'part', 'whole'],
    ['day N|tag N|d N|day N s', 'day', null],
    ['tomorrow|morgen', 'rel', 'tomorrow'], ['today|heute', 'rel', 'today'],
    ['along the whole route|along the route|along route|along the way|on the route|whole route|the whole route|whole trip|the whole trip|entire route|entire trip|unterwegs|entlang der route|entlang|auf der route|on the way|en route', 'where', 'route'],
    ['in this map view|this map view|map view|in this view|in view|on the map|on screen', 'where', 'view'],
    ['from here|starting here|starting from here|start here|here|hier|von hier|ab hier', 'here', true],
    ['open on|open|opened|geoffnet|offen|opening hours|hours', 'open', true],
    ['sunday|sundays|sun|sonntag|sonntags', 'wd', 'sun'], ['monday|mondays|mon|montag', 'wd', 'mon'], ['tuesday|tuesdays|tue|dienstag', 'wd', 'tue'],
    ['wednesday|wednesdays|wed|mittwoch', 'wd', 'wed'], ['thursday|thursdays|thu|donnerstag', 'wd', 'thu'], ['friday|fridays|fri|freitag', 'wd', 'fri'], ['saturday|saturdays|sat|samstag', 'wd', 'sat'],
    ['within N km|within N kilometres|within N kilometers|N km|innerhalb N km|innerhalb von N km|im umkreis von N km|N kilometres|N kilometers', 'within', null],
    ['road bike|roadbike|road|rennrad', 'bike', 'road'], ['gravel bike|gravelbike|gravel', 'bike', 'gravel'], ['mtb|mountain bike|mountainbike', 'bike', 'mtb'],
    ['touring bike|touring|trekking|tourenrad|trekkingrad', 'bike', 'touring'],
    ['shortest|kurzeste|fastest|quickest|short', 'goal', 'shortest'], ['least climbing|least climb|flattest|flat|wenig hohenmeter|flach|easy', 'goal', 'leastclimb'],
    ['least unpaved|paved|asphalt|on roads|roads only|asphaltiert|tarmac', 'goal', 'leastunpaved'], ['most climbing|most climb|hilliest|hilly', 'goal', 'mostclimb'], ['balanced|ausgewogen', 'goal', 'balanced'],
    ['route|ride|way|tour|weg|strecke|fahrt|cycle|ride to|route to|route up|way to', 'routeword', true],
    ['via|uber|through|by way of|stopping at|stop at|visit|visiting|with a stop at|passing', 'prep', 'via'],
    ['at|in|near|nearby|next to|around|close to|bei|nahe|in der nahe von|um', 'prep', 'near'],
    ['to|towards|up|nach|zum|zur|bis|up to', 'prep', 'to'],
    ['from|von|ab', 'prep', 'from'],
    ['gaps|gap|longest stretch|longest stretches|stretches without|stretch without|lucken|lucke|no water for|without water|without', 'gaps', true],
    ['bike|bicycle|fahrrad|by bike|cycling|riding|places|place|spots|spot|somewhere|anywhere|options|option|nearest|closest|list|show me|find me|is there|are there|where is|where are|gibt es|wo ist', 'noise', true],
  ];
  const GLUE = new Set('a an the of on in at for to from and me please show find search look looking i want need some any along with by'.split(' '));
  const GENERIC = new Set('hotel camping col cime saint lac de du la le les sur arc fountain im mont cenis grand petit'.split(' '));

  const norm = (s) => s.toLowerCase().normalize('NFD').replace(/[̀-ͯ]/g, '').replace(/ß/g, 'ss')
    .replace(/[-–/,']/g, ' ').replace(/[^a-z0-9&.\s]/g, '').replace(/\bst\.?(?=\s|$)/g, 'saint').replace(/\./g, '').trim();
  const toks = (s) => (norm(s) ? norm(s).split(/\s+/) : []);

  const rules = WORDS.flatMap(([ph, field, value]) => ph.split('|').map((p) => ({ t: p.split(' '), field, value })))
    .sort((a, b) => b.t.length - a.t.length);
  const kindWords = rules.filter((r) => r.field === 'kind' && r.t.length === 1 && r.t[0].length >= 5);

  // Optimal string alignment distance, capped at 2: enough for a typo of one letter.
  function dist1(a, b) {
    if (Math.abs(a.length - b.length) > 1) return 2;
    const d = [];
    for (let i = 0; i <= a.length; i++) { d[i] = [i]; for (let j = 1; j <= b.length; j++) d[i][j] = i ? 0 : j; }
    for (let i = 1; i <= a.length; i++) for (let j = 1; j <= b.length; j++) {
      let c = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) c = Math.min(c, d[i - 2][j - 2] + 1);
      d[i][j] = c;
    }
    return d[a.length][b.length];
  }

  function matchRule(t, i, r) {
    for (let k = 0; k < r.t.length; k++) {
      const w = t[i + k]; if (w === undefined) return 0;
      if (r.t[k] === 'N' ? !/^\d+$/.test(w) : r.t[k] !== w) return 0;
    }
    return r.t.length;
  }

  const placeNames = (p) => [p.name, ...(p.alias || [])].map(toks);
  function matchPlace(t, i, ctx) {
    let best = null;
    for (const p of DATA.PLACES) for (const nt of placeNames(p)) {
      for (let j = 0; j < nt.length; j++) {
        let L = 0; while (t[i + L] !== undefined && nt[j + L] === t[i + L]) L++;
        const whole = L === nt.length && j === 0;
        const ok = L >= 2 || (L === 1 && (whole || (t[i].length >= 5 && !GENERIC.has(t[i]))));
        if (!ok) continue;
        const score = L * 10 + (whole ? 5 : 0) + (p.region === ctx.region || p.map === ctx.map || p.map === 'd4' && ctx.map === 'alps' ? 1 : 0);
        if (!best || score > best.score) best = { p, L, score };
      }
    }
    return best;
  }

  function parse(text, ctx) {
    const t = toks(text), n = t.length, used = new Array(n).fill(false), m = [];
    // 1. places first, longest span wins over a kind word of the same length
    for (let i = 0; i < n; i++) {
      if (used[i]) continue;
      const pl = matchPlace(t, i, ctx);
      let rule = null, rl = 0;
      for (const r of rules) { const L = matchRule(t, i, r); if (L > rl) { rule = r; rl = L; } }
      if (pl && pl.L >= Math.max(2, rl) || pl && pl.L === 1 && rl === 0) { m.push({ field: 'place', value: pl.p, i, L: pl.L }); used.fill(true, i, i + pl.L); i += pl.L - 1; continue; }
      if (rule) { m.push({ field: rule.field, value: rule.value, i, L: rl, t: t.slice(i, i + rl) }); used.fill(true, i, i + rl); i += rl - 1; continue; }
      if (t[i].length >= 5 && !/^\d+$/.test(t[i])) {              // a typo of one letter in a kind word
        const kw = kindWords.find((r) => dist1(t[i], r.t[0]) <= 1);
        if (kw) { m.push({ field: 'kind', value: kw.value, i, L: 1, typo: true }); used[i] = true; }
      }
    }
    // 2. leftover runs: glue is dropped silently, anything else is an "off" chip
    const orig = text.trim().split(/\s+/).filter(Boolean), src = [], off = [];
    orig.forEach((w, k) => toks(w).forEach(() => src.push(k)));
    for (let i = 0; i < n;) {
      if (used[i]) { i++; continue; }
      let j = i; while (j < n && !used[j]) j++;
      if (!t.slice(i, j).every((w) => GLUE.has(w))) off.push(src.length === n ? orig.slice(src[i], src[j - 1] + 1).join(' ') : t.slice(i, j).join(' '));
      i = j;
    }
    // 3. assemble the request
    const get = (f) => m.filter((x) => x.field === f);
    const req = { kinds: [], off, part: null, day: null, needDate: false, within: null, open: null, bike: null, goal: null, here: false, routeword: false, gaps: false };
    for (const k of get('kind')) for (const kk of (k.value === 'sleep' ? ['campsite', 'lodging'] : [k.value])) if (!req.kinds.includes(kk)) req.kinds.push(kk);
    const day = get('day')[0]; if (day) req.day = +day.t.find((w) => /^\d+$/.test(w));
    const rel = get('rel')[0];
    if (rel && !req.day) { if (ctx.hasDates) req.day = ctx.today + (rel.value === 'tomorrow' ? 1 : 0); else { req.needDate = true; req.relWord = rel.t.join(' '); } }
    if (req.day && !DATA.DAYS.some((d) => d.n === req.day && !d.rest)) req.day = null;
    req.part = (get('part')[0] || {}).value || null;
    const within = get('within')[0]; if (within) req.within = +within.t.find((w) => /^\d+$/.test(w));
    req.here = get('here').length > 0;
    req.routeword = get('routeword').length > 0;
    req.gaps = get('gaps').length > 0;
    req.bike = (get('bike')[0] || {}).value || null;
    req.goal = (get('goal')[0] || {}).value || null;
    const wd = get('wd')[0];
    if (get('open').length || wd) req.open = wd ? { wd: wd.value } : { onDay: true };
    const w = get('where')[0]; req.whereWord = w ? w.value : null;
    // places with their role: the preposition just before the span
    req.places = get('place').map((pm) => {
      const prev = m.find((x) => x.i + x.L === pm.i && x.field === 'prep');
      return { place: pm.value, role: prev ? prev.value : (pm.i === 0 && n === pm.L ? 'bare' : 'bare') };
    });
    const withRole = (r) => (req.places.find((p) => p.role === r) || {}).place || null;
    req.via = withRole('via'); req.to = withRole('to'); req.near = withRole('near'); req.from = withRole('from');
    req.bare = (req.places.find((p) => p.role === 'bare') || {}).place || null;
    if (!req.near && get('prep').some((p) => p.value === 'near') && !req.kinds.length && req.bare) { req.near = req.bare; req.bare = null; }
    return decide(req, ctx);
  }

  // The request kind, in a fixed order of precedence.
  function decide(req, ctx) {
    const content = req.kinds.length || req.day || req.part || req.within || req.open || req.here || req.routeword || req.gaps || req.bike || req.goal || req.whereWord || req.places.length;
    if (req.via) req.kind = 'change', req.change = { type: 'visit', place: req.via };
    else if (req.part === 'end' && req.day && (req.near || req.to) && !req.kinds.length) req.kind = 'change', req.change = { type: 'dayend', day: req.day, place: req.near || req.to };
    else if (req.gaps) req.kind = 'gaps', req.gapKind = req.kinds.includes('shop') ? 'shop' : 'water';
    else if (req.kinds.length) req.kind = 'places';
    else if (req.to || (req.routeword && (req.bare || req.near)) || (req.bare && ctx.plan.kind === 'new' && !req.bare.ambiguous && !req.off.length && !req.day))
      req.kind = 'route', req.to = req.to || req.bare || req.near;
    else if ((req.bare || req.near) && !req.off.length) req.kind = 'place', req.place = req.bare || req.near;
    else if (req.routeword && req.here && !req.off.length) req.kind = 'route';
    else { req.kind = 'none'; req.place = req.bare || req.near || null; req.partial = !!content; }
    if (req.kind === 'route') { req.bike = req.bike || ctx.plan.bike; req.goal = req.goal || ctx.plan.goal; }
    return req;
  }

  // The "where" of a places request: sentence words first, then the pointed chip, then the map view.
  function where(req, ctx) {
    if (req.near) return { type: 'near', place: req.near };
    if (req.day) return { type: 'day', day: req.day, part: req.part || 'whole' };
    if (req.whereWord) return { type: req.whereWord };
    if (req.here && ctx.plan.here) return { type: 'here' };
    if (ctx.pointed) return req.part ? Object.assign({}, ctx.pointed, { part: req.part }) : ctx.pointed;
    if (req.part && ctx.selDay) return { type: 'day', day: ctx.selDay, part: req.part };
    if (req.needDate) return { type: 'needDate' };
    return { type: 'view' };
  }

  function kindsLabel(kinds) {
    const l = kinds.map((k, i) => (i ? DATA.KINDS[k].label.toLowerCase() : DATA.KINDS[k].label));
    return l.length <= 1 ? l.join('') : l.slice(0, -1).join(', ') + ' and ' + l[l.length - 1];
  }

  return { parse, where, kindsLabel, toks };
})();
