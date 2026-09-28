/* The box: the input with the pointed "where" chip inside it, the understood chips under it, and
   one picker per editable chip. A chip never shows words the rider did not type: the chips are
   built from the request, the sentence stays in the field (faint once a chip was edited). */

const Box = (() => {
  const { esc, icon, count } = Answers;
  let input, pointedEl, countEl, clearEl, h = {};

  function init(handlers) {
    h = handlers;
    input = document.getElementById('qin'); pointedEl = document.getElementById('pointed'); countEl = document.getElementById('qcount'); clearEl = document.getElementById('qclear');
    input.addEventListener('input', () => h.onInput(input.value));
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); h.onSubmit(); input.blur(); }
      if (e.key === 'Escape') input.blur();
      if (e.key === 'Backspace' && !input.value && pointedEl.firstChild) h.onRemovePointed();
    });
    input.addEventListener('focus', () => h.onFocus()); input.addEventListener('blur', () => h.onBlur());
    clearEl.addEventListener('click', () => h.onClear());
  }
  function sync(S) {
    if (input.value !== S.text) input.value = S.text;
    const qb = input.parentElement;
    qb.classList.toggle('edited', S.edited); qb.classList.toggle('focus', S.focused);
    countEl.textContent = S.focused ? `${S.text.length}/80` : '';
    clearEl.hidden = !(S.text || S.pointed) || S.focused;
    const p = S.pointed && Answers.whereInfo(S.pointed, S);
    const ph = p ? `<span class="chip where pointed" data-act="pickField" data-field="where">${esc(p.label)}${p.k ? ` <span class="k">${esc(p.k)}</span>` : ''}<button class="xb" data-act="removePointed" title="Remove">${icon('i-close')}</button></span>` : '';
    if (pointedEl.innerHTML !== ph) pointedEl.innerHTML = ph;
  }
  const focus = () => { input.focus(); const n = input.value.length; try { input.setSelectionRange(n, n); } catch (e) {} };

  // ---- chip specs from the request ----
  function chips(req, ans, S) {
    const out = [], K = DATA.KINDS;
    if (!req) return out;
    const chev = true;
    if (req.kind === 'places') {
      const wi = ans.wi;
      out.push({ field: 'what', cls: 'what', icon: K[req.kinds[0]].icon, label: Parser.kindsLabel(req.kinds), edit: chev });
      out.push({ field: 'where', cls: wi.need ? 'need' : 'where', icon: wi.need ? 'i-calendar' : null, label: wi.label, k: wi.need ? 'needs a date' : wi.k, edit: !wi.need, act: wi.need ? 'setDates' : null });
      if (ans.within != null) out.push({ field: 'within', cls: 'filter', label: `within ${ans.within} km`, edit: true });
      if (ans.open) out.push({ field: 'open', cls: 'filter', label: ans.open.label, edit: true, removable: true });
      else if (ans.need) out.push({ field: 'open', cls: 'need', icon: 'i-calendar', label: `Open on Day ${req.where.day || ''}`.trim(), k: 'needs a date', edit: true });
      else if (S.removed.open) out.push({ field: 'open', cls: 'off', label: `Open on ${DATA.WEEKDAYS[S.removed.open.wd] || 'the day'}`, act: 'restore', data: 'open' });
    } else if (req.kind === 'route') {
      out.push({ field: null, cls: 'what', icon: 'i-route', label: 'Route' });
      out.push({ field: null, cls: 'where', label: `To ${req.to.name}${req.to.kind === 'summit' ? ' summit' : ''}` });
      out.push({ field: null, cls: 'where', icon: 'i-here', label: 'From here', k: DATA.place(ans.from ? ans.from.id : S.here).name });
      out.push({ field: 'bike', cls: 'filter', icon: 'i-bike', label: DATA.BIKES[req.bike], edit: true });
    } else if (req.kind === 'change' && req.change.type === 'dayend') {
      out.push({ field: null, cls: 'what', icon: 'i-tent', label: `End Day ${req.change.day}` });
      out.push({ field: null, cls: 'where', label: `at ${req.change.place.name}`, k: req.change.place.km != null ? `km ${req.change.place.km}` : null });
    } else if (req.kind === 'change') {
      out.push({ field: null, cls: 'what', icon: 'i-flag', label: 'Add a visit' });
      out.push({ field: null, cls: 'where', label: req.change.place.name });
    } else if (req.kind === 'gaps') {
      out.push({ field: null, cls: 'what', icon: ans.icon, label: ans.label });
      out.push({ field: null, cls: 'where', label: 'Whole trip' });
    } else if (req.kind === 'place') {
      out.push({ field: null, cls: 'what', icon: 'i-search', label: 'Place' });
      out.push({ field: null, cls: 'where', label: req.place.name, k: req.place.region === 'Black Forest' && req.place.ambiguous ? 'near Freiburg' : null });
    } else if (req.kind === 'none' && req.place) {
      out.push({ field: null, cls: 'what', icon: 'i-search', label: 'Place' });
      out.push({ field: null, cls: 'where', label: req.place.name });
    }
    if (req.needDate && req.kind !== 'places') out.push({ field: null, cls: 'need', icon: 'i-calendar', label: req.relWord, k: 'needs a date', act: 'setDates' });
    for (const o of req.off) out.push({ field: null, cls: 'off', label: o, k: req.kind === 'places' ? '· not understood, ignored' : null });
    return out;
  }
  // One row of chips: it wraps on the website and scrolls sideways on the phone.
  function chipsHtml(specs, S) {
    if (!specs.length) return '';
    const c = specs.map((s) => {
      const tag = s.edit || s.act ? 'button' : 'span', on = s.edit && S.picker === s.field;
      const act = s.act ? `data-act="${s.act}" data-field="${s.data || s.field || ''}"` : s.edit ? `data-act="pickField" data-field="${s.field}"` : '';
      return `<${tag} class="chip ${s.cls} ${on ? 'on' : ''} ${s.removable ? 'rm' : ''}" ${act} data-chip="${s.field || ''}">${s.icon ? icon(s.icon) : ''}${esc(s.label)}${s.k ? ` <span class="k">${esc(s.k)}</span>` : ''}${s.edit ? icon('i-chev-d', 'chev') : ''}${s.removable ? `<i class="xb" data-act="removeFilter" data-field="${s.field}" title="Turn off">${icon('i-close')}</i>` : ''}</${tag}>`;
    }).join('');
    return `<div class="chips"><span class="lbl">Understood as</span>${c}</div>`;
  }
  // The "Find" row: a where is set but no what.
  const findHtml = () => `<div class="chips"><span class="lbl">Find</span>` + [['sleep', 'i-tent', 'Places to sleep'], ['water', 'i-drop', 'Water'], ['shop', 'i-cart', 'Shops'], ['bikeshop', 'i-wrench', 'Bike shops']]
    .map(([k, ic, l]) => `<button class="chip what" data-act="find" data-k="${k}">${icon(ic)}${l}</button>`).join('') + `</div>`;

  // ---- pickers ----
  const seg = (items, cls = '') => `<div class="seg2 ${cls}">${items.map((i) => `<button class="${i.on ? 'on' : ''}" data-act="${i.act}" ${i.data}>${i.label}${i.small != null ? `<small>${i.small}</small>` : ''}</button>`).join('')}</div>`;
  function pickerHtml(field, req, ans, S) {
    if (!field || !req) return '';
    let inner = '';
    if (field === 'what') {
      const fam = req.kinds.every((k) => DATA.SLEEP.includes(k)) ? DATA.SLEEP : DATA.SUPPLY;
      inner = `<div class="tg">${fam.map((k) => { const on = req.kinds.includes(k), n = count(req, S, { kinds: [k] }); return `<button class="${on ? 'on' : ''}" data-act="pickKind" data-k="${k}"><i>${on ? '<svg class="ck" viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7"/></svg>' : ''}</i>${DATA.KINDS[k].label.replace(/s$/, '')}<small>${n}</small></button>`; }).join('')}</div>`;
    } else if (field === 'where') {
      const w = req.where || S.pointed || {};
      inner = seg([{ label: 'This map view', on: w.type === 'view', act: 'pickWhere', data: 'data-type="view"' }, { label: 'Along the route', on: w.type === 'route', act: 'pickWhere', data: 'data-type="route"' }]);
      if (S.plan().kind === 'trip') {
        inner += `<div class="seg2 days">${DATA.DAYS.filter((d) => !d.rest).map((d) => `<button class="${w.day === d.n ? 'on' : ''}" data-act="pickWhere" data-type="day" data-n="${d.n}">Day ${d.n}</button>`).join('')}</div>`;
        if (w.type === 'day') inner += seg([['start', 'Start'], ['middle', 'Middle'], ['end', 'End'], ['whole', 'Whole day']].map(([p, l]) => ({ label: l, on: w.part === p, act: 'pickWhere', data: `data-type="day" data-n="${w.day}" data-part="${p}"` })));
        inner += `<button class="prow-pick" data-act="pickOnMap">${icon('i-route')}Pick on the map or the profile${icon('i-chev-r')}</button>`;
      }
    } else if (field === 'within') {
      inner = seg(Answers.RINGS.map((r) => ({ label: `${r} km`, small: count(req, S, { within: r }), on: ans.within === r, act: 'pickWithin', data: `data-km="${r}"` })));
    } else if (field === 'open') {
      const wd = ans.open ? ans.open.wd : null;
      inner = seg(Object.entries(DATA.WEEKDAYS).map(([k, l]) => ({ label: l.slice(0, 3), on: wd === k, act: 'pickOpen', data: `data-wd="${k}"` })));
      inner += seg([{ label: 'Any day', on: false, act: 'pickOpen', data: 'data-wd="any"' }]);
      if (!S.dates) inner += `<div class="note-pick">Open on ${req.where && req.where.day ? `Day ${req.where.day}` : 'the day'} needs a date<button class="btn" data-act="setDates">Set trip dates</button></div>`;
    } else if (field === 'bike') {
      inner = seg(Object.entries(DATA.BIKES).map(([k, l]) => ({ label: l, on: (req ? req.bike : S.plan().bike) === k, act: 'pickBike', data: `data-bike="${k}"` })));
      inner += seg(['balanced', 'shortest', 'leastclimb', 'leastunpaved'].map((g) => ({ label: DATA.GOALS[g], on: (req ? req.goal : S.plan().goal) === g, act: 'pickBike', data: `data-goal="${g}"` })), 'two');
    }
    return inner ? `<div class="pick" data-pick="${field}">${inner}</div>` : '';
  }
  // Place the picker's arrow under its chip.
  function placeArrow(container) {
    const pick = container.querySelector('.pick'), chip = pick && container.querySelector(`[data-chip="${pick.dataset.pick}"]`);
    if (pick && chip) pick.style.setProperty('--ax', chip.offsetLeft - chip.parentElement.scrollLeft + chip.offsetWidth / 2 + 'px');
  }
  return { init, sync, focus, chips, chipsHtml, findHtml, pickerHtml, placeArrow, get input() { return input; } };
})();
