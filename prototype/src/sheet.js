/* The phone sheet: three detents (low = the box and one line, medium, high). A touch drag
   anywhere on the sheet moves it; at the high detent the body scrolls natively and only a
   downward drag from its top pulls the sheet. A mouse drags it by the handle or the header. */

const Sheet = (() => {
  let el, body, heights = { low: 150, medium: 460, high: 640 }, detent = 'medium', height = 460, onDetent = null, H = 844, suppressClickUntil = 0;
  const ORDER = ['low', 'medium', 'high'];

  function setHeight(h, animate) {
    el.style.transition = animate ? '' : 'none';
    height = h; el.style.height = h + 'px';
    el.classList.toggle('hide-pin', h < heights.medium - 60);
    if (animate) requestAnimationFrame(() => (el.style.transition = ''));
  }
  function setDetent(d, animate = true, silent) {
    const changed = d !== detent; detent = d;
    setHeight(heights[d], animate);
    el.dataset.detent = d; body.scrollTop = 0;
    if (changed && !silent && onDetent) onDetent(d);
  }
  // Measure the detents from the layout: the low one ends with the first .oneline element.
  function measure(appHeight, topInset) {
    H = appHeight;
    const one = el.querySelector('.oneline'), sab = parseFloat(getComputedStyle(el).paddingBottom) || 0;
    heights.low = one ? one.offsetTop + one.offsetHeight + 10 + sab : 150;
    heights.medium = Math.round(H * 0.56);
    heights.high = H - topInset - 96;
    if (heights.low > heights.medium - 40) heights.low = heights.medium - 40;
    setHeight(heights[detent], false);
  }
  function snap(target, vy) {
    let d = nearest(target);
    if (Math.abs(vy) > 0.45) {                                       // a fling goes one detent from where it started
      const dir = vy < 0 ? 1 : -1, next = ORDER[Math.max(0, Math.min(2, ORDER.indexOf(detent) + dir))];
      if ((ORDER.indexOf(d) - ORDER.indexOf(next)) * dir < 0) d = next;
    }
    setDetent(d, true);
  }
  const nearest = (h) => ORDER.reduce((a, b) => (Math.abs(heights[b] - h) < Math.abs(heights[a] - h) ? b : a));
  const rubber = (h) => (h > heights.high ? heights.high + (h - heights.high) * 0.25 : h < heights.low ? heights.low - (heights.low - h) * 0.25 : h);

  function bind() {
    let y0 = 0, h0 = 0, pending = false, dragging = false, ignore = false, inBody = false, samples = [];
    el.addEventListener('touchstart', (e) => {
      const t = e.touches[0]; y0 = t.clientY; h0 = height; samples = [[performance.now(), t.clientY]];
      inBody = body.contains(e.target); pending = true; dragging = false;
      ignore = inBody && detent === 'high' && body.scrollTop > 0;
      el.style.transition = 'none';
    }, { passive: true });
    el.addEventListener('touchmove', (e) => {
      if (ignore) return;
      const y = e.touches[0].clientY, dy = y - y0;
      if (pending) {
        if (inBody && detent === 'high' && body.scrollTop <= 0 && dy < 0) { ignore = true; return; }   // a scroll, not a drag
        if (Math.abs(dy) < 3) return;
        pending = false; dragging = true;
      }
      e.preventDefault();
      samples.push([performance.now(), y]); if (samples.length > 6) samples.shift();
      setHeight(rubber(h0 - dy), false);
    }, { passive: false });
    const end = () => {
      if (!dragging) { el.style.transition = ''; return; }
      dragging = false; suppressClickUntil = performance.now() + 350;
      const [t1, y1] = samples[0], [t2, y2] = samples[samples.length - 1], vy = t2 > t1 ? (y2 - y1) / (t2 - t1) : 0;
      snap(height, vy);
    };
    el.addEventListener('touchend', end); el.addEventListener('touchcancel', end);
    el.addEventListener('click', (e) => { if (performance.now() < suppressClickUntil) { e.stopPropagation(); e.preventDefault(); } }, true);
    // mouse: the handle and the header only
    el.addEventListener('pointerdown', (e) => {
      if (e.pointerType !== 'mouse' || !e.target.closest('.grab, .oneline, .head-drag')) return;
      e.preventDefault(); const ys = e.clientY, hs = height; let moved = false; el.style.transition = 'none';
      const mv = (ev) => { const dy = ev.clientY - ys; if (Math.abs(dy) > 3) moved = true; setHeight(rubber(hs - dy), false); };
      const upp = () => { window.removeEventListener('pointermove', mv); window.removeEventListener('pointerup', upp); if (moved) snap(height, 0); else el.style.transition = ''; };
      window.addEventListener('pointermove', mv); window.addEventListener('pointerup', upp);
    });
    el.querySelector('.grab').addEventListener('click', () => setDetent(detent === 'high' ? 'medium' : 'high'));
  }

  function init(sheetEl, h) { el = sheetEl; body = el.querySelector('.body'); onDetent = h && h.onDetent; bind(); el.dataset.detent = detent; }
  return { init, measure, setDetent, get detent() { return detent; }, get height() { return height; }, heights };
})();
