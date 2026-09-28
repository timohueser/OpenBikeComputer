/* The phone sheet: two heights, collapsed (the head and the profile) and middle (plus the box and
   one list), and the full state while the box is focused (CSS lays that one out). A touch drag on
   the handle, the head or the profile moves it, except a horizontal drag the profile claimed
   (Profile.touch); a scroll of the list never does. A mouse drags it by the handle or the head. A
   picker adds its height to the middle (extra), so the list stays in view. */

const Sheet = (() => {
  let el, body, heights = { collapsed: 250, middle: 540 }, detent = 'collapsed', height = 250, extra = 0, full = false, onDetent = null, suppressClickUntil = 0;

  function setHeight(h, animate) {
    el.style.transition = animate ? '' : 'none';
    height = h; if (!full) el.style.height = h + 'px';
    if (animate) requestAnimationFrame(() => (el.style.transition = ''));
  }
  const target = (d) => heights[d] + (d === 'middle' ? extra : 0);
  function setDetent(d, animate = true, silent) {
    const changed = d !== detent; detent = d;
    setHeight(target(d), animate);
    el.dataset.detent = d; body.scrollTop = 0;
    if (changed && !silent && onDetent) onDetent(d);
  }
  // Leaving the full state pins the current height first, so the next setDetent animates from it.
  function setFull(v) {
    if (v === full) return;
    full = v; el.style.transition = 'none';
    if (v) { el.classList.add('full'); el.style.height = ''; }
    else { el.style.height = el.offsetHeight + 'px'; el.classList.remove('full'); void el.offsetHeight; }
    el.style.transition = '';
  }
  // Measure the heights from the layout: the collapsed one ends with the profile.
  function measure(appHeight) {
    const pf = el.querySelector('#pfslot'), sab = parseFloat(getComputedStyle(el).paddingBottom) || 0;
    heights.middle = Math.round(appHeight * 0.64);
    if (pf.offsetHeight) heights.collapsed = Math.min(pf.offsetTop + pf.offsetHeight + 10 + sab, heights.middle - 80);
    if (!full && Math.abs(height - target(detent)) > 0.5) setHeight(target(detent), false);
  }
  function setExtra(px) { if (px === extra) return; extra = px; if (detent === 'middle' && !full) setHeight(target('middle'), true); }
  function snap(h, vy) {
    let d = Math.abs(target('middle') - h) < Math.abs(target('collapsed') - h) ? 'middle' : 'collapsed';
    if (Math.abs(vy) > 0.45) d = vy < 0 ? 'middle' : 'collapsed';                 // a fling decides
    setDetent(d, true);
  }
  const rubber = (h) => { const hi = target('middle'), lo = target('collapsed'); return h > hi ? hi + (h - hi) * 0.25 : h < lo ? lo - (lo - h) * 0.25 : h; };
  const grabs = (t) => !full && !!t.closest('.grab, #head, #pfslot');

  function bind() {
    let y0 = 0, h0 = 0, pending = false, dragging = false, ignore = true, samples = [];
    el.addEventListener('touchstart', (e) => {
      const t = e.touches[0]; y0 = t.clientY; h0 = height; samples = [[performance.now(), t.clientY]];
      ignore = !grabs(e.target); pending = true; dragging = false;
      if (!ignore) el.style.transition = 'none';
    }, { passive: true });
    el.addEventListener('touchmove', (e) => {
      if (ignore || Profile.touch === 'pending' || Profile.touch === 'h') return;
      const y = e.touches[0].clientY, dy = y - y0;
      if (pending) { if (Math.abs(dy) < 3) return; pending = false; dragging = true; }
      e.preventDefault();
      samples.push([performance.now(), y]); if (samples.length > 6) samples.shift();
      setHeight(rubber(h0 - dy), false);
    }, { passive: false });
    const end = () => {
      if (!dragging) { el.style.transition = ''; return; }
      dragging = false; suppressClickUntil = performance.now() + 350;
      const [t1, y1] = samples[0], [t2, y2] = samples[samples.length - 1];
      snap(height, t2 > t1 ? (y2 - y1) / (t2 - t1) : 0);
    };
    el.addEventListener('touchend', end); el.addEventListener('touchcancel', end);
    el.addEventListener('click', (e) => { if (performance.now() < suppressClickUntil) { e.stopPropagation(); e.preventDefault(); } }, true);
    el.addEventListener('pointerdown', (e) => {
      if (e.pointerType !== 'mouse' || full || !e.target.closest('.grab, #head')) return;
      e.preventDefault(); const ys = e.clientY, hs = height; let moved = false; el.style.transition = 'none';
      const mv = (ev) => { const dy = ev.clientY - ys; if (Math.abs(dy) > 3) moved = true; setHeight(rubber(hs - dy), false); };
      const upp = () => { window.removeEventListener('pointermove', mv); window.removeEventListener('pointerup', upp); if (moved) { snap(height, 0); suppressClickUntil = performance.now() + 350; } else el.style.transition = ''; };
      window.addEventListener('pointermove', mv); window.addEventListener('pointerup', upp);
    });
    el.querySelector('.grab').addEventListener('click', () => { if (!full) setDetent(detent === 'middle' ? 'collapsed' : 'middle'); });
  }

  function init(sheetEl, h) { el = sheetEl; body = el.querySelector('.body'); onDetent = h && h.onDetent; bind(); el.dataset.detent = detent; }
  return { init, measure, setDetent, setFull, setExtra, get detent() { return detent; }, get height() { return height; }, get full() { return full; }, heights };
})();
