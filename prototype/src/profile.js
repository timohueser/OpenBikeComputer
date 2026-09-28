/* The profile: one component for the website's bottom panel, the phone's medium detent and the
   phone in landscape. It shows a km window set from outside (the map drives it; the profile never
   zooms or scrolls itself), the days as coloured areas, every day end as a handle with a 44 px
   grab band the full height of the plot, the scrubber, and the answer's ticks. A drag of a handle
   moves only the overlay here and reports the km; the app updates the rest. On touch, a first
   movement that is more vertical than horizontal belongs to the sheet (Profile.touch says so). */

const Profile = (() => {
  let el = null, spec = null, key = null, plot = null, svg = null, live = null, touch = null;
  const pct = (km) => (((km - spec.win[0]) / (spec.win[1] - spec.win[0])) * 100).toFixed(2) + '%';
  const yPct = (e) => ((1 - e / spec.max) * 100).toFixed(1) + '%';
  const samples = () => (spec.src.use ? Line.profile(spec.src.use + '-line') : spec.src.samples);
  const elevAt = (km) => Line.elevAt(samples(), km);
  const esc = Answers.esc;

  // The plot, per day: a solid light shade of the day colour under the line, the line in the full colour.
  function build() {
    const y0 = spec.max === 3000 ? 0 : 150, vh = 300 - y0, ends = spec.ends || [], days = spec.days.filter((d) => !d.rest);
    const clips = spec.days.map((d) => `<clipPath id="pfc${d.i}"><rect x="${d.start}" y="${y0}" width="${Math.max(0, d.end - d.start)}" height="${vh}"/></clipPath>`).join('');
    const shape = (kind, extra) => (spec.src.use ? `<use href="#${spec.src.use}-${kind}" ${extra}/>` : `<path d="${kind === 'area' ? areaD() : Line.pathD(samples().map(([k, e]) => [k, 300 - e / 10]))}" ${extra}/>`);
    const areas = days.map((d) => shape('area', `clip-path="url(#pfc${d.i})" class="pf-area ${d.sel ? 'sel' : ''} ${d.flat ? 'flat' : ''}" style="--dc:${d.color}" data-act="selDay" data-n="${d.n}" data-i="${d.i}"`)).join('');
    const lines = days.map((d) => shape('line', `clip-path="url(#pfc${d.i})" class="pf-line" style="--dc:${d.color}"`)).join('');
    const marks = ['closure', 'gap'].flatMap((k) => (spec.marks[k] || []).map(([a, b]) => `<rect class="pf-${k}" x="${a}" y="${y0}" width="${b - a}" height="${vh}"/>`)).join('');
    const rules = ends.map((e) => `<line class="pf-rule" data-i="${e.i}" x1="${e.km}" x2="${e.km}" y1="${y0}" y2="300"/>`).join('');
    const grid = spec.max === 3000 ? `<line class="prof-grid" x1="0" x2="9999" y1="100" y2="100"/><line class="prof-grid" x1="0" x2="9999" y1="200" y2="200"/>` : `<line class="prof-grid" x1="0" x2="9999" y1="225" y2="225"/>`;
    el.innerHTML = `<span class="pl"></span><div class="pf-nums"></div>
      <span class="pl ax">${spec.max === 3000 ? '3,000 m' : '1,500 m'}<br><span class="lo">0 m</span></span>
      <div class="pf-plot"><svg preserveAspectRatio="none"><defs>${clips}</defs>${marks}${grid}<g class="pf-areas">${areas}</g>${lines}${rules}<line class="pf-scrub" y1="${y0}" y2="300" style="display:none"/></svg>
      <div class="pf-over"></div><span class="pf-read" hidden></span></div>` +
      (spec.rows ? `<span class="pl">Surface</span><div class="band" data-row="surface"></div><span class="pl">Snow history</span><div class="band" data-row="snow"></div>` : '');
    plot = el.querySelector('.pf-plot'); svg = plot.querySelector('svg');
    bindPlot();
  }
  const areaD = () => { const S = samples(); return Line.pathD(S.map(([k, e]) => [k, 300 - e / 10])) + `L${S[S.length - 1][0]} 300L${S[0][0]} 300Z`; };
  const bandRow = (k) => (spec.marks[k] || []).map(([a, b]) => `<i class="${k}" style="left:${pct(a)};width:${(((b - a) / (spec.win[1] - spec.win[0])) * 100).toFixed(2)}%"></i>`).join('');

  // Everything that moves with a window, a drag or a selection.
  function update() {
    const [a, b] = spec.win, y0 = spec.max === 3000 ? 0 : 150, ends = live ? spec.ends.map((e) => (e.i === live.i ? { ...e, km: live.km } : e)) : spec.ends || [];
    const restAfter = live && spec.days[live.i + 1] && spec.days[live.i + 1].rest;
    const days = live ? spec.days.map((d) => ({ ...d, start: d.i === live.i + 1 || (restAfter && d.i === live.i + 2) ? live.km : d.start, end: d.i === live.i || (restAfter && d.i === live.i + 1) ? live.km : d.end })) : spec.days;
    svg.setAttribute('viewBox', `${a} ${y0} ${Math.max(0.1, b - a)} ${300 - y0}`);
    days.forEach((d) => { const r = svg.querySelector(`#pfc${d.i} rect`); if (r) { r.setAttribute('x', d.start); r.setAttribute('width', Math.max(0, d.end - d.start)); } });
    svg.querySelectorAll('.pf-area').forEach((a) => { const d = days.find((x) => x.i === +a.dataset.i); if (d) a.classList.toggle('sel', !!d.sel); });
    svg.querySelectorAll('.pf-rule').forEach((r) => { const e = ends.find((x) => x.i === +r.dataset.i); if (e) { r.setAttribute('x1', e.km); r.setAttribute('x2', e.km); } });
    el.querySelector('.pf-nums').innerHTML = days.filter((d) => d.end > a && d.start < b).map((d) => { const m = (Math.max(a, d.start) + Math.min(b, d.end)) / 2; return !d.n ? '' : d.rest ? `<i style="left:${pct(d.start)}">rest</i>` : `<b class="${d.sel ? 'sel' : ''}" style="left:${pct(m)};color:${d.color}" data-act="selDay" data-n="${d.n}">${d.n}</b>`; }).join('');
    const scrub = svg.querySelector('.pf-scrub'); scrub.style.display = spec.scrub == null ? 'none' : ''; if (spec.scrub != null) { scrub.setAttribute('x1', spec.scrub); scrub.setAttribute('x2', spec.scrub); }
    let over = (spec.ticks || []).filter((t) => t.km >= a && t.km <= b).map((t) => `<i class="pf-tick ${t.sel ? 'sel' : ''}" style="left:${pct(t.km)}" data-act="selRow" data-id="${esc(t.id)}"></i>`).join('');
    over += ends.filter((e) => e.km >= a && e.km <= b).map((e) => { const big = e.sel || (live && live.i === e.i); return `<div class="pf-band" data-i="${e.i}" style="left:${pct(e.km)}"></div><b class="pf-end ${big ? 'big' : ''}" data-i="${e.i}" style="left:${pct(e.km)};top:${yPct(elevAt(e.km))};background:${e.color || 'var(--ink)'}" data-act="pointEnd" data-n="${e.n}">${big ? e.n : ''}</b>`; }).join('');
    if (spec.scrub != null && spec.scrub >= a && spec.scrub <= b) over += `<i class="pf-pt" style="left:${pct(spec.scrub)};top:${yPct(elevAt(spec.scrub))}"></i>`;
    plot.querySelector('.pf-over').innerHTML = over;
    const read = plot.querySelector('.pf-read'); read.hidden = !spec.reading || spec.scrub == null; if (spec.reading) { read.textContent = spec.reading; const p = (spec.scrub - a) / (b - a); read.style.left = p > 0.6 ? 'auto' : pct(spec.scrub); read.style.right = p > 0.6 ? (100 - parseFloat(pct(spec.scrub))).toFixed(2) + '%' : 'auto'; }
    if (spec.rows) { el.querySelector('[data-row="surface"]').innerHTML = `<i class="paved" style="left:0;width:100%"></i>${bandRow('unknown')}${bandRow('unpaved')}`; el.querySelector('[data-row="snow"]').innerHTML = bandRow('snow'); }
  }
  function render(container, s) {
    const k = [s.key, s.src.use || 'samples', s.days.length, s.rows, (s.ends || []).length, s.max].join('|');
    if (container !== el || k !== key) { el = container; spec = s; key = k; build(); } else spec = s;
    update();
  }
  const kmAt = (clientX) => { const r = plot.getBoundingClientRect(); return spec.win[0] + ((clientX - r.left) / r.width) * (spec.win[1] - spec.win[0]); };

  function bindPlot() {
    let drag = null, scrubbing = false, moved = false;
    plot.addEventListener('pointerdown', (e) => {
      if (e.button) return;
      const band = e.target.closest('.pf-band'), tick = e.target.closest('.pf-tick'), end = e.target.closest('.pf-end');
      if (tick) return;
      moved = false; touch = e.pointerType === 'touch' ? 'pending' : 'h';
      const i = band ? +band.dataset.i : end ? +end.dataset.i : null;
      const km0 = i != null ? (spec.ends.find((x) => x.i === i) || {}).km : null;
      drag = { i, x0: e.clientX, y0: e.clientY, km0, id: e.pointerId, target: band || end || plot };
      if (i == null) scrubbing = true;
      try { drag.target.setPointerCapture(e.pointerId); } catch (x) {}
    });
    plot.addEventListener('pointermove', (e) => {
      if (!drag) { if (e.pointerType === 'mouse' && spec.handlers.onScrub && !e.target.closest('.pf-band, .pf-end')) spec.handlers.onScrub(kmAt(e.clientX), 'hover'); return; }
      const dx = e.clientX - drag.x0, dy = e.clientY - drag.y0;
      if (touch === 'pending') { if (Math.hypot(dx, dy) < 6) return; touch = Math.abs(dy) > Math.abs(dx) ? 'v' : 'h'; }
      if (touch === 'v') return;
      moved = true;
      if (drag.i != null) { const km = drag.km0 + (dx / plot.getBoundingClientRect().width) * (spec.win[1] - spec.win[0]); live = { i: drag.i, km: spec.handlers.onEnd(drag.i, km, 'move') }; update(); }
      else if (scrubbing) spec.handlers.onScrub(kmAt(e.clientX), 'drag');
    });
    const up = (e) => {
      if (!drag) return;
      const d = drag, vertical = touch === 'v'; drag = null; scrubbing = false; touch = null;
      if (vertical) return;                                            // the sheet's drag, not ours
      if (d.i != null && live) { const km = live.km; live = null; spec.handlers.onEnd(d.i, km, 'end'); }
      else if (d.i != null && !moved) spec.handlers.onEnd(d.i, d.km0, 'tap');
      else if (!moved && e.type === 'pointerup' && spec.handlers.onScrub) spec.handlers.onScrub(kmAt(e.clientX), 'tap');
    };
    plot.addEventListener('pointerup', up); plot.addEventListener('pointercancel', up);
    plot.addEventListener('click', (e) => { const t = e.target.closest('.pf-tick'); if (t && spec.handlers.onTick) spec.handlers.onTick(t.dataset.id); });
  }
  return { render, elevAt, get touch() { return touch; }, get live() { return live; } };
})();
