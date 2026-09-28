/* The map: one SVG with a view {map, cx, cy, scale} (scale = screen px per map unit). Pan by
   drag, zoom by wheel, pinch, double tap and the +/− buttons; animated framing; a switch between
   the alps overview and the Day 4 map by zoom. Markers and labels sit in map coordinates and keep
   their pixel size through --mk / --lk (= 1 / scale). A drag that starts on a leg of the line
   (data-leg) moves a point instead of the map; in draw mode a drag is a stroke. Not a map engine. */

const MapView = (() => {
  const LIMITS = { bf: [0.3, 3], alps: [0.4, 3.2], d4: [0.3, 3] };
  const K_D4 = 4.14;                                   // d4 units per alps unit (geometric mean of x 3.81, y 4.5)
  let el, svg, baseUse, layers, view = { map: 'alps', cx: 350, cy: 450, scale: 1 }, anim = null, handlers = {}, drawMode = false;
  const api = { insets: () => ({}) };
  const bboxCache = {};

  const size = () => ({ W: el.clientWidth || 1, H: el.clientHeight || 1 });
  const toLL = (map, x, y) => { const m = DATA.MAPS[map]; return { lon: m.lon0 + x / m.w * m.dlon, lat: m.lat0 - y / m.h * m.dlat }; };
  const toXY = (map, lon, lat) => { const m = DATA.MAPS[map]; return { x: (lon - m.lon0) / m.dlon * m.w, y: (m.lat0 - lat) / m.dlat * m.h }; };
  // A position {map, x, y} in the coordinates of another map (alps ↔ d4; bf never overlaps).
  function convert(pos, map) { if (pos.map === map) return { x: pos.x, y: pos.y }; const ll = toLL(pos.map, pos.x, pos.y); return toXY(map, ll.lon, ll.lat); }
  const kmBetween = (map, a, b) => { const m = DATA.MAPS[map]; return Math.hypot((a.x - b.x) * m.kmx, (a.y - b.y) * m.kmy); };

  function apply() {
    const { W, H } = size(), w = W / view.scale, h = H / view.scale;
    svg.setAttribute('viewBox', `${view.cx - w / 2} ${view.cy - h / 2} ${w} ${h}`);
    svg.style.setProperty('--mk', 1 / view.scale);
    svg.style.setProperty('--lk', 1.1 / view.scale);
    handlers.move && handlers.move();
  }
  function setMap(map) {
    if (view.map === map) return;
    const c = convert({ map: view.map, x: view.cx, y: view.cy }, map);
    view.scale *= (map === 'd4' ? 1 / K_D4 : view.map === 'd4' ? K_D4 : 1);
    view.map = map; view.cx = c.x; view.cy = c.y;
    baseUse.setAttribute('href', '#map-' + map);
    handlers.change && handlers.change();
  }
  // Zoom by the rider switches the alps overview and the Day 4 map (with hysteresis).
  function maybeSwitch() {
    if (view.map === 'alps' && view.scale > 2.3) {
      const ins = api.insets(), ll = toLL('alps', view.cx, view.cy + ((ins.t || 0) - (ins.b || 0)) / 2 / view.scale), m = DATA.MAPS.d4;
      if (ll.lon > m.lon0 && ll.lon < m.lon0 + m.dlon && ll.lat < m.lat0 && ll.lat > m.lat0 - m.dlat) setMap('d4');
    } else if (view.map === 'd4' && view.scale < 0.38) setMap('alps');
  }
  function stopAnim() { if (anim) cancelAnimationFrame(anim); anim = null; }
  function animateTo(t) {
    stopAnim();
    const from = { ...view }, t0 = performance.now(), dur = 420;
    const step = (now) => {
      const u = Math.min(1, (now - t0) / dur), e = 1 - Math.pow(1 - u, 3);
      view.cx = from.cx + (t.cx - from.cx) * e; view.cy = from.cy + (t.cy - from.cy) * e;
      view.scale = Math.exp(Math.log(from.scale) + (Math.log(t.scale) - Math.log(from.scale)) * e);
      apply();
      anim = u < 1 ? requestAnimationFrame(step) : null;
    };
    anim = requestAnimationFrame(step);
  }
  function screenToMap(sx, sy) { const r = el.getBoundingClientRect(), { W, H } = size(); return { map: view.map, x: view.cx + (sx - r.left - W / 2) / view.scale, y: view.cy + (sy - r.top - H / 2) / view.scale }; }
  // The screen position of a map position, relative to the map element.
  function toScreen(pos) { const c = convert(pos, view.map), { W, H } = size(); return { x: W / 2 + (c.x - view.cx) * view.scale, y: H / 2 + (c.y - view.cy) * view.scale }; }
  function zoomAt(f, sx, sy, animate) {
    const p = screenToMap(sx, sy), s = view.scale, [lo, hi] = LIMITS[view.map], ns = Math.min(hi, Math.max(lo, s * f));
    const t = { cx: p.x - (p.x - view.cx) * s / ns, cy: p.y - (p.y - view.cy) * s / ns, scale: ns };
    if (animate) animateTo(t); else { Object.assign(view, t); apply(); }
    maybeSwitch();
  }
  function zoomBy(f) { const r = el.getBoundingClientRect(); zoomAt(f, r.left + r.width / 2, r.top + r.height / 2, true); }

  // Frame a bbox {x, y, w, h} of `map` into the area left free by insets {t, b} (px), animated.
  function frame(map, box, insets = {}, opts = {}) {
    const { W, H } = size(), t = insets.t || 0, b = insets.b || 0, pad = opts.pad || 40, fw = W - pad * 2, fh = Math.max(80, H - t - b - pad * 2);
    let scale = Math.min(fw / Math.max(box.w, 1), fh / Math.max(box.h, 1));
    const [lo, hi] = LIMITS[map]; scale = Math.min(opts.max || hi, Math.max(lo, scale));
    const target = { cx: box.x + box.w / 2, cy: box.y + box.h / 2 + (b - t) / 2 / scale, scale };
    if (map !== view.map) { setMap(map); Object.assign(view, target); apply(); handlers.change && handlers.change(); return; }
    if (opts.instant) { Object.assign(view, target); apply(); } else animateTo(target);
  }
  function panTo(pos, insets = {}) {
    const c = convert(pos, view.map), t = insets.t || 0, b = insets.b || 0;
    animateTo({ cx: c.x, cy: c.y + (b - t) / 2 / view.scale, scale: view.scale });
  }
  function bboxOfPath(id) {
    if (bboxCache[id]) return bboxCache[id];
    const d = document.getElementById(id).getAttribute('d'), nums = d.match(/-?\d+\.?\d*/g).map(Number);
    let x0 = 1e9, y0 = 1e9, x1 = -1e9, y1 = -1e9;
    for (let i = 0; i + 1 < nums.length; i += 2) { x0 = Math.min(x0, nums[i]); x1 = Math.max(x1, nums[i]); y0 = Math.min(y0, nums[i + 1]); y1 = Math.max(y1, nums[i + 1]); }
    return (bboxCache[id] = { x: x0, y: y0, w: x1 - x0, h: y1 - y0 });
  }
  const bboxOfPts = (pts) => { let x0 = 1e9, y0 = 1e9, x1 = -1e9, y1 = -1e9; for (const [x, y] of pts) { x0 = Math.min(x0, x); x1 = Math.max(x1, x); y0 = Math.min(y0, y); y1 = Math.max(y1, y); } return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }; };
  const union = (a, b) => { const x = Math.min(a.x, b.x), y = Math.min(a.y, b.y); return { x, y, w: Math.max(a.x + a.w, b.x + b.w) - x, h: Math.max(a.y + a.h, b.y + b.h) - y }; };
  // The visible rectangle in map units, minus the insets; plus a helper for "in this map view".
  function viewRect(insets = {}) {
    const { W, H } = size(), t = insets.t || 0, b = insets.b || 0;
    return { x: view.cx - W / 2 / view.scale, y: view.cy - H / 2 / view.scale + t / view.scale, w: W / view.scale, h: (H - t - b) / view.scale };
  }
  function inRect(pos, r, growKm = 0) {
    const p = convert(pos, view.map), m = DATA.MAPS[view.map], gx = growKm / m.kmx, gy = growKm / m.kmy;
    return p.x >= r.x - gx && p.x <= r.x + r.w + gx && p.y >= r.y - gy && p.y <= r.y + r.h + gy;
  }
  const widthKm = () => (size().W / view.scale) * DATA.MAPS[view.map].kmx;

  // ---- input ----
  function bindInput() {
    const ptrs = new Map(); let drag = null, pinch = null, lastTap = 0, lastTapAt = null, stroke = null;
    svg.addEventListener('pointerdown', (e) => {
      if (e.button) return;
      svg.setPointerCapture(e.pointerId); ptrs.set(e.pointerId, { x: e.clientX, y: e.clientY }); stopAnim();
      if (ptrs.size === 1) {
        const leg = e.target.closest('[data-leg]');
        drag = { sx: e.clientX, sy: e.clientY, cx: view.cx, cy: view.cy, moved: false, leg: leg && !drawMode ? +leg.dataset.leg : null };
        stroke = drawMode ? [] : null; pinch = null;
      } else if (ptrs.size === 2) { const [a, b] = [...ptrs.values()]; pinch = { d0: Math.hypot(a.x - b.x, a.y - b.y), s0: view.scale, mx: (a.x + b.x) / 2, my: (a.y + b.y) / 2, cx: view.cx, cy: view.cy }; drag = null; stroke = null; }
    });
    svg.addEventListener('pointermove', (e) => {
      if (!ptrs.has(e.pointerId)) return;
      ptrs.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (pinch && ptrs.size >= 2) {
        const [a, b] = [...ptrs.values()], d = Math.hypot(a.x - b.x, a.y - b.y), mx = (a.x + b.x) / 2, my = (a.y + b.y) / 2;
        const [lo, hi] = LIMITS[view.map], ns = Math.min(hi, Math.max(lo, pinch.s0 * d / pinch.d0));
        const r = el.getBoundingClientRect(), { W, H } = size();
        const px = pinch.cx + (pinch.mx - r.left - W / 2) / pinch.s0, py = pinch.cy + (pinch.my - r.top - H / 2) / pinch.s0;
        view.cx = px - (mx - r.left - W / 2) / ns; view.cy = py - (my - r.top - H / 2) / ns; view.scale = ns; apply();
      } else if (drag) {
        const dx = e.clientX - drag.sx, dy = e.clientY - drag.sy;
        if (!drag.moved && Math.hypot(dx, dy) > 4) { drag.moved = true; if (drag.leg != null) handlers.legDrag(drag.leg, screenToMap(e.clientX, e.clientY), 'start'); }
        if (!drag.moved) return;
        if (stroke) { stroke.push(screenToMap(e.clientX, e.clientY)); handlers.draw(stroke, 'move'); }
        else if (drag.leg != null) handlers.legDrag(drag.leg, screenToMap(e.clientX, e.clientY), 'move');
        else { view.cx = drag.cx - dx / view.scale; view.cy = drag.cy - dy / view.scale; apply(); }
      }
    });
    const up = (e) => {
      if (!ptrs.has(e.pointerId)) return;
      ptrs.delete(e.pointerId);
      if (pinch) { maybeSwitch(); if (ptrs.size === 1) { const p = [...ptrs.values()][0]; drag = { sx: p.x, sy: p.y, cx: view.cx, cy: view.cy, moved: true, leg: null }; } else if (!ptrs.size) pinch = null; return; }
      if (drag && drag.moved && stroke) handlers.draw(stroke, 'end');
      else if (drag && drag.moved && drag.leg != null) handlers.legDrag(drag.leg, screenToMap(e.clientX, e.clientY), 'end');
      else if (drag && !drag.moved && ptrs.size === 0 && e.type === 'pointerup') {
        const now = performance.now(), hit = document.elementFromPoint(e.clientX, e.clientY), act = hit && hit.closest('[data-act]');
        if (act && svg.contains(act)) handlers.tap(act.dataset, screenToMap(e.clientX, e.clientY));
        else if (now - lastTap < 320 && lastTapAt && Math.hypot(e.clientX - lastTapAt.x, e.clientY - lastTapAt.y) < 24) { zoomAt(2, e.clientX, e.clientY, true); lastTap = 0; }
        else { handlers.tapEmpty(screenToMap(e.clientX, e.clientY)); lastTap = now; lastTapAt = { x: e.clientX, y: e.clientY }; }
      }
      drag = null; stroke = null;
    };
    svg.addEventListener('pointerup', up); svg.addEventListener('pointercancel', up);
    svg.addEventListener('wheel', (e) => { e.preventDefault(); zoomAt(Math.exp(-e.deltaY * (e.ctrlKey ? 0.012 : 0.0016)), e.clientX, e.clientY, false); }, { passive: false });
    new ResizeObserver(apply).observe(el);
  }

  function init(container, h) {
    el = container; handlers = h || {};
    el.innerHTML = `<svg id="mapsvg" xmlns="http://www.w3.org/2000/svg"><g id="map-base"><use href="#map-alps"/></g><g id="map-routes"></g><g id="map-marks"></g></svg><span class="attribution">© OpenStreetMap</span>`;
    svg = el.querySelector('svg'); baseUse = svg.querySelector('#map-base use');
    layers = { routes: svg.querySelector('#map-routes'), marks: svg.querySelector('#map-marks') };
    bindInput(); apply();
  }
  const setLayers = (routes, marks) => { layers.routes.innerHTML = routes; layers.marks.innerHTML = marks; };

  const setDraw = (v) => { drawMode = v; el.classList.toggle('drawing', v); };
  return Object.assign(api, { init, setLayers, frame, panTo, zoomBy, setMap, convert, kmBetween, bboxOfPath, bboxOfPts, union, viewRect, inRect, widthKm, toScreen, screenToMap, setDraw, get view() { return view; } });
})();
