const { chromium } = require('playwright');
const path = require('path');
(async () => {
  const b = await chromium.launch();
  const url = 'file://' + path.resolve(process.argv[2]);
  const errs = [];
  const p = await b.newPage({ viewport: { width: 1280, height: 900 } });
  p.on('pageerror', (e) => errs.push(e.message));
  p.on('console', (m) => { if (m.type() === 'error' || m.type() === 'warning') errs.push(m.text()); });
  await p.goto(url); await p.waitForTimeout(300);
  const names = await p.evaluate(() => JSON.parse(document.getElementById('system-map').textContent).systems.map((s) => s.name));
  const rows = [];
  for (const n of names) for (const open of [false, true]) {
    await p.evaluate((n) => { location.hash = n; }, n);
    await p.waitForTimeout(20);
    if (open) { await p.evaluate(() => document.querySelectorAll('.problems details').forEach((d) => (d.open = true))); await p.waitForTimeout(60); }
    rows.push(await p.evaluate(([n, open]) => {
      const r = (s) => document.querySelector(s)?.getBoundingClientRect();
      const mid = (x) => x.top + x.height / 2;
      const c = r('.center-card'), u = r('.col.uses .stack'), d = r('.col.usedby .stack'), pr = r('.problems');
      return { n, open, nb: document.querySelectorAll('.nb').length, dU: +(mid(c) - mid(u)).toFixed(1), dD: +(mid(c) - mid(d)).toFixed(1),
        gap: pr ? +(pr.top - c.bottom).toFixed(1) : null, room: pr ? +(document.documentElement.scrollHeight - (pr.bottom + scrollY)).toFixed(1) : null,
        past: pr ? Math.round(pr.bottom - Math.max(r('.col.uses').bottom, r('.col.usedby').bottom)) : null,
        vb: (() => { const f = document.getElementById('focus'), v = f.querySelector('svg.wires').viewBox.baseVal; return v.width === f.clientWidth && v.height === f.clientHeight; })() };
    }, [n, open]));
  }
  const bad = rows.filter((x) => Math.abs(x.dU) > 1 || Math.abs(x.dD) > 1 || (x.gap !== null && Math.abs(x.gap - 14) > 1) || (x.room !== null && x.room < 0) || !x.vb);
  console.log('rows', rows.length, 'bad', JSON.stringify(bad));
  console.log('panel extends past columns (px):', JSON.stringify(rows.filter((x) => x.past > 0).map((x) => [x.n, x.open ? 'open' : 'closed', x.nb + ' nb', x.past])));
  // Card must not move when the panel grows.
  await p.evaluate(() => { location.hash = 'device-map-baking'; }); await p.waitForTimeout(50);
  const y0 = await p.evaluate(() => document.querySelector('.center-card').getBoundingClientRect().top + scrollY);
  await p.evaluate(() => document.querySelectorAll('.problems details').forEach((d) => (d.open = true))); await p.waitForTimeout(80);
  const y1 = await p.evaluate(() => document.querySelector('.center-card').getBoundingClientRect().top + scrollY);
  console.log('dmb card top closed/open', y0, y1);
  // Double-click and single click in the overview.
  await p.evaluate(() => { location.hash = 'map-reading'; }); await p.waitForTimeout(30);
  await p.click('.tabs [data-view="overview"]');
  await p.click('.chip[data-name="routing"]');
  console.log('single click:', await p.evaluate(() => [location.hash, document.querySelector('.tabs [aria-selected="true"]').dataset.view]));
  await p.dblclick('.chip[data-name="search"]');
  console.log('double click:', await p.evaluate(() => [location.hash, document.querySelector('.tabs [aria-selected="true"]').dataset.view]));
  await p.click('.tabs [data-view="overview"]');
  const box = await (await p.$('.chip[data-name="trailview"]')).boundingBox();
  await p.mouse.click(box.x + 20, box.y + 8, { clickCount: 1 }); await p.mouse.click(box.x + 20, box.y + 8, { clickCount: 2 });
  console.log('mouse double click:', await p.evaluate(() => [location.hash, document.querySelector('.tabs [aria-selected="true"]').dataset.view]));
  // 400px: no horizontal scroll on any system.
  await p.setViewportSize({ width: 400, height: 900 });
  let wide = [];
  for (const n of names) { await p.evaluate((n) => { location.hash = n; }, n); await p.waitForTimeout(15);
    const sw = await p.evaluate(() => document.documentElement.scrollWidth); if (sw > 400) wide.push([n, sw]); }
  console.log('400px wide pages', JSON.stringify(wide), 'errors', JSON.stringify(errs));
  await b.close();
})();
