// One scripted run through the flows of the briefs: node check/run.js [url]
const puppeteer = require('puppeteer-core');
const path = require('path');
const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const URL = process.argv[2] || 'file://' + path.resolve(__dirname, '..', 'index.html');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
let page;

async function step(name, fn) {
  try { const v = await fn(); results.push([true, name, v === undefined ? '' : String(v)]); }
  catch (e) { results.push([false, name, e.message.split('\n')[0]]); }
}
const ev = (fn, ...args) => page.evaluate(fn, ...args);
const text = (sel) => ev((s) => (document.querySelector(s) || {}).textContent || '', sel);
const count = (sel) => ev((s) => document.querySelectorAll(s).length, sel);
const has = (sel) => ev((s) => !!document.querySelector(s), sel);
const visible = (sel) => ev((s) => { const e = document.querySelector(s); return !!e && e.offsetHeight > 0; }, sel);
const S = (expr) => ev((e) => eval(e), expr);
const rect = (sel) => ev((s) => { const r = document.querySelector(s).getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height, top: r.top }; }, sel);
async function type(t, enter = true) { await ev(() => { App.A.clear(); App.render(); }); await page.click('#qin'); await page.type('#qin', t, { delay: 3 }); if (enter) await page.keyboard.press('Enter'); await sleep(250); }
const assert = (c, m) => { if (!c) throw new Error('assert: ' + m); };
const includes = (a, b) => { assert(String(a).includes(b), `"${String(a).slice(0, 90)}" does not include "${b}"`); };
async function mouseDrag(x0, y0, x1, y1) { await page.mouse.move(x0, y0); await page.mouse.down(); for (let i = 1; i <= 8; i++) await page.mouse.move(x0 + (x1 - x0) * i / 8, y0 + (y1 - y0) * i / 8); await page.mouse.up(); await sleep(300); }
const mapPoint = (pos) => ev((p) => { const s = MapView.toScreen(p), r = document.getElementById('map').getBoundingClientRect(); return { x: r.left + s.x, y: r.top + s.y }; }, pos);
const openPlan = async (id, tap) => { await page[tap ? 'tap' : 'click'](`#plans [data-id="${id}"]`); await sleep(500); };

async function web() {
  page = await browser.newPage();
  page.on('pageerror', (e) => results.push([false, 'pageerror', e.message]));
  page.on('console', (m) => { if (m.type() === 'error') results.push([false, 'console.error', m.text()]); });
  await page.setViewport({ width: 1440, height: 900 });
  await page.goto(URL, { waitUntil: 'load' }); await sleep(500);

  await step('plans page under the site header: three rows with sketches; the Alps row opens the planner', async () => {
    assert(await visible('#site'), 'site header'); assert((await count('#plans .prow2')) === 3, 'rows'); assert((await count('#plans .sk svg')) === 3, 'sketches'); includes(await text('#plans'), 'Schwarzwald Gravel, 3 days');
    await openPlan('alps'); assert(!(await visible('#plans')), 'plans hidden'); includes(await text('#bar .back'), 'Routes and trips'); includes(await text('#bar .plan'), 'Alps'); assert(!(await has('#bar [data-menu="plan"]')), 'no plan switcher');
  });
  await step('rest: days list, bars, profile with 8 handles, a coloured line per day', async () => { assert((await count('.day')) === 10, 'days'); assert((await count('.day .bar')) === 9, 'bars'); assert((await count('#prof .pf-band')) === 8, 'handles'); assert((await count('#prof .pf-line')) === 9, 'lines ' + (await count('#prof .pf-line'))); });
  await step('expand Day 4: points and legs; leg menu → Straight; undo', async () => {
    await page.click('.day[data-n="4"] .dchev'); await sleep(200); assert((await count('.dpts .ptrow')) === 4, 'points ' + (await count('.dpts .ptrow'))); assert((await count('.dpts .legrow')) === 3, 'legs');
    await page.click('.dpts .legrow'); await sleep(150); includes(await text('.menu'), 'Straight'); await page.click('.menu [data-mode="straight"]'); await sleep(300);
    assert(await has('#map-routes .rt-line.straight'), 'straight on map'); includes(await text('.dpts .legrow'), 'Straight');
    await page.click('#bar [data-act="undo"]'); await sleep(200); assert(!(await has('#map-routes .rt-line.straight')), 'undone');
  });
  await step('select Day 4: map d4 with the other days still drawn, the profile window follows, Day 4 highlighted', async () => {
    await page.click('.day[data-n="4"]'); await sleep(700); assert((await S('MapView.view.map')) === 'd4', 'map d4'); const w = await S('App.S.win'); assert(w[0] > 150 && w[1] < 400 && w[1] - w[0] < 200, 'window ' + w);
    assert((await count('#map-routes .rt-line')) > 6, 'other days on d4: ' + (await count('#map-routes .rt-line'))); assert(await has('#prof .pf-area.sel'), 'area sel'); assert((await count('#prof .pf-band')) >= 1, 'handle in window');
  });
  await step('drag the Day 4 end on the profile → Day 4 shorter, Day 5 longer; undo', async () => {
    const b = await rect('#prof .pf-band[data-i="3"]'); await mouseDrag(b.x, b.y, b.x - 120, b.y);
    const e = await S('App.S.mut().ends[3]'); assert(e < 316, 'end moved ' + e); const k4 = await text('.day[data-n="4"] .km'); assert(parseInt(k4) < 104, 'day 4 km ' + k4); const k5 = await text('.day[data-n="5"] .km'); assert(parseInt(k5) > 53, 'day 5 km ' + k5);
    assert((await S('App.S.hist.length')) === 1, 'one history step'); await page.click('#bar [data-act="undo"]'); await sleep(200); includes(await text('.day[data-n="4"] .km'), '104');
    return `end ${Math.round(e)} · Day 4 ${k4} · Day 5 ${k5}`;
  });
  await step('day menu: Split → 11 days, Join → back; Rename', async () => {
    await page.click('.day.sel .dmore'); await sleep(150); includes(await text('.menu'), 'Split this day'); await page.click('.menu [data-act="daySplit"]'); await sleep(300); assert((await count('.day')) === 11, 'split ' + (await count('.day')));
    await page.click('.day[data-n="4"]'); await sleep(100); await page.click('.day.sel .dmore'); await sleep(150); await page.click('.menu [data-act="dayJoin"]'); await sleep(300); assert((await count('.day')) === 10, 'joined');
    await page.click('.day[data-n="4"]'); await sleep(100); await page.click('.day.sel .dmore'); await sleep(150); await page.click('.menu [data-act="dayRename"]'); await sleep(200); assert(await has('input.rename'), 'input'); await page.keyboard.type(' · Iseran day'); await page.keyboard.press('Enter'); await sleep(200); includes(await text('.day[data-n="4"] .t'), 'Iseran day');
    await ev(() => { App.A.undo(); App.A.undo(); App.A.undo(); App.render(); });
  });
  await step('tap a day end on the profile → where chip + Find row', async () => { const b = await rect('#prof .pf-band[data-i="3"]'); await page.mouse.click(b.x, b.y); await sleep(200); includes(await text('#pointed'), 'End of Day 4'); includes(await text('#chips'), 'Find'); await ev(() => { App.A.clear(); App.render(); }); });
  await step('"campsites end of day 4": chips, rows, action; Le Petit Nice → off the line → Out and back', async () => {
    await type('campsites end of day 4'); includes(await text('#chips'), 'within 5 km'); assert((await count('.row.prow')) === 5, 'rows'); includes(await text('#pin'), 'End Day 4 here'); assert(await has('#prof .pf-tick.sel'), 'tick');
    await page.click('.row.prow[data-id="camping-le-petit-nice-saint-michel-de-maurienne"]'); await sleep(200); await page.click('#pin [data-act="endDay"]'); await sleep(300);
    assert(await has('#callout .co-row[data-via="outback"]'), 'asks how'); includes(await text('#callout'), 'Through it'); await page.click('#callout [data-via="outback"]'); await sleep(400);
    includes(await text('.day[data-n="4"] .km'), '89'); includes(await text('.day[data-n="4"] .t'), 'Camping Le Petit Nice'); includes(await text('#body'), 'Day 4 now ends at'); assert(await has('#map-routes .rt-spur'), 'spur');
    await page.click('#body [data-act="undo"]'); await sleep(200); includes(await text('.day[data-n="4"] .km'), '104');
  });
  await step('sleep pin on the map → callout → End Day 4 here → Through it; undo', async () => {
    await page.click('.day[data-n="4"]'); await sleep(700); assert(await has('.mk.pin'), 'pins when zoomed'); const p = await mapPoint({ map: 'd4', x: 434, y: 458 }); await page.mouse.click(p.x, p.y); await sleep(250);
    includes(await text('#callout'), "Camping de l'Arc"); includes(await text('#callout'), 'End Day 4 here'); await page.click('#callout [data-act="coEnd"]'); await sleep(200); await page.click('#callout [data-via="through"]'); await sleep(400);
    includes(await text('.day[data-n="4"] .t'), "Camping de l'Arc"); await page.click('#body [data-act="undo"]'); await sleep(200); includes(await text('.day[data-n="4"] .t'), 'Valloire');
  });
  await step('"end day 4 at saint-michel": proposal, Apply, days update; undo/redo', async () => {
    await type('end day 4 at saint-michel'); const b = await text('#body'); includes(b, '104 km→88 km'); includes(b, '69 km'); includes(b, 'Télégraphe'); assert(await has('.mk.newEnd'), 'ghost');
    await page.click('#pin [data-act="apply"]'); await sleep(300); includes(await text('.day[data-n="4"] .km'), '88'); includes(await text('.day[data-n="5"] .t'), 'Saint-Michel');
    await page.click('#bar [data-act="undo"]'); await sleep(200); includes(await text('.day[data-n="4"] .km'), '104'); await page.click('#bar [data-act="redo"]'); await sleep(200); includes(await text('.day[data-n="4"] .km'), '88'); await page.click('#bar [data-act="undo"]'); await sleep(200);
  });
  await step('map: tap the line → Add point callout → pass-here point on Day 4; type → Visit; Remove; undo', async () => {
    await page.click('.day[data-n="4"]'); await sleep(700); const p = await mapPoint({ map: 'd4', x: 640, y: 380 }); await page.mouse.click(p.x, p.y); await sleep(250);
    includes(await text('#callout'), 'Add point'); await page.click('#callout [data-act="coAdd"]'); await sleep(300); assert((await S('App.S.mut().points.length')) === 1, 'point added');
    await ev(() => { App.S.expanded.add(4); App.render(); }); await sleep(200); assert((await count('.dpts .ptrow')) === 5, 'in the list'); await page.click('.dpts .ptrow[data-pid^="a"]'); await sleep(200); includes(await text('#callout'), 'Remove point');
    await page.click('#callout [data-kind="visit"]'); await sleep(200); assert((await S('App.S.mut().points[0].kind')) === 'visit', 'visit'); await page.click('#callout [data-act="coRemove"]'); await sleep(200); assert((await S('App.S.mut().points.length')) === 0, 'removed');
    await page.click('#bar [data-act="undo"]'); await sleep(100); assert((await S('App.S.mut().points.length')) === 1, 'undo restores'); await ev(() => { App.A.undo(); App.A.undo(); App.render(); });
  });
  await step('"water gaps" → Add marker; "shops open sunday day 4"; empty; partly; not understood', async () => {
    await type('water gaps'); includes(await text('#body'), 'No mapped water for 29 km'); const n0 = await S('App.S.mut().points.length'); await page.click('#pin [data-act="addMarker"]'); await sleep(200); assert((await S('App.S.mut().points.length')) === n0 + 1, 'marker'); await page.click('#body [data-act="undo"]'); await sleep(100);
    await type('shops open sunday day 4'); includes(await text('#head'), '3 open on Sunday'); await type('bike shop end of day 4'); includes(await text('#body'), 'No bike shops within 5 km'); await type('campsites with a pool end of day 4'); includes(await text('.chip.off'), 'with a pool'); await type('is the galibier worth it'); includes(await text('#body'), 'Col du Galibier');
  });
  await step('parser variations', async () => {
    const out = [];
    for (const [t, want] of [['day 4 campsites', 'places:day:whole'], ['Zeltplatz Ende Tag 4', 'places:day:end'], ['campsits end of day 4', 'places:day:end'], ['tomorrow campsites', 'places:day:whole:4'], ['end day 4 at a campsite', 'endcamp'], ['pharmacies along route', 'places:route']]) {
      await type(t, false); const r = await S('(function(){const r=App.S.req,w=r.where||{};return [r.kind,w.type,w.part,w.day,r.within,(App.S.ans&&App.S.ans.actions&&App.S.ans.actions.primary.label)||""].join(":")})()');
      assert(want === 'endcamp' ? r.startsWith('places:day:end') && r.endsWith('End Day 4 here') : r.includes(want), `${t} → ${r}`); out.push(`${t} → ${r}`);
    }
    return out.length + ' ok';
  });
  await step('Routes and trips → new plan: "Titisee" → 3 options → Use this route → two points, one leg, amber profile', async () => {
    await page.click('#bar .back'); await sleep(200); assert(await visible('#plans'), 'plans page'); await openPlan('new'); includes(await text('#body'), 'Tap the map');
    await type('Titisee'); assert((await count('.row.orow')) === 3, 'options'); includes(await text('#pin'), 'Use this route'); await page.click('.row.orow[data-id="leastclimb"]'); await sleep(200);
    await page.click('#pin [data-act="useRoute"]'); await sleep(500); assert((await S('App.S.mut().pts.length')) === 2, 'pts'); assert((await S('App.S.mut().legs[0].path')) === 'route-bf-leastclimb', 'leg path'); includes(await text('#body'), 'Least climbing'); assert((await count('.ptrow')) === 2, 'point rows'); assert(await has('#prof .pf-area.flat'), 'profile');
  });
  await step('build: tap map → Add point; tap point → Visit; leg → Straight; drag line → point inserted; Drawn stroke; undo all', async () => {
    const p = await mapPoint({ map: 'bf', x: 700, y: 470 }); await page.mouse.click(p.x, p.y); await sleep(250); includes(await text('#callout'), 'Add point'); await page.click('#callout [data-act="coAdd"]'); await sleep(300);
    assert((await S('App.S.mut().pts.length')) === 3, '3 pts'); assert((await S('App.S.mut().legs.length')) === 2, '2 legs'); includes(await text('#head'), 'estimated');
    const q = await mapPoint({ map: 'bf', x: 700, y: 470 }); await page.mouse.click(q.x, q.y); await sleep(250); includes(await text('#callout'), 'Remove point'); await page.click('#callout [data-kind="visit"]'); await sleep(200); assert((await S('App.S.mut().pts[2].kind')) === 'visit', 'visit');
    await page.click('#callout [data-act="coClose"]'); await sleep(100); await page.click('.dpts .legrow, .legrow'); await sleep(150); await page.click('.menu [data-mode="straight"]'); await sleep(300); assert((await S('App.S.mut().legs[0].mode')) === 'straight', 'straight');
    const mid = await ev(() => { const P = Route.legPts(App.S.mut(), 1), m = P[Math.floor(P.length / 2)], s = MapView.toScreen({ map: 'bf', x: m[0], y: m[1] }), r = document.getElementById('map').getBoundingClientRect(); return { x: r.left + s.x, y: r.top + s.y }; });
    await mouseDrag(mid.x, mid.y, mid.x + 40, mid.y - 60); assert((await S('App.S.mut().pts.length')) === 4, 'inserted ' + (await S('App.S.mut().pts.length'))); assert((await S('App.S.mut().pts[2].kind')) === 'shape', 'shape');
    await ev(() => { App.A.legMenu({ key: '2', leg: '2' }); App.render(); }); await sleep(100); await page.click('.menu [data-mode="drawn"]'); await sleep(200); assert(await S('App.S.drawing !== null'), 'draw mode'); includes(await text('#head'), 'Draw the leg');
    const mr = await rect('#map'); await mouseDrag(mr.x, mr.y - 40, mr.x + 90, mr.y + 30); assert((await S('App.S.mut().legs[2].mode')) === 'drawn', 'drawn'); assert(await has('#map-routes .rt-line.drawn'), 'drawn on map');
    const n = await S('App.S.hist.length'); for (let i = 0; i < n; i++) { await page.click('#bar [data-act="undo"]'); await sleep(60); } assert((await S('App.S.mut().legs.length')) === 0, 'undone to the start'); return `${n} undo steps`;
  });
  await step('"Kandel" → place; Route from here → 20.8 km; bike picker', async () => { await type('Kandel'); includes(await text('#body'), 'Other places with this name'); await page.click('#pin [data-act="routeFrom"]'); await sleep(300); includes(await text('#body'), '20.8 km'); await page.click('[data-chip="bike"]'); await sleep(100); await page.click('.pick [data-bike="gravel"]'); await sleep(200); includes(await text('#chips'), 'Gravel'); });
  await step('Import menu → the imported plan → "via Hotel Krone St. Peter" → Apply → visit; leg callout says not re-routed; undo', async () => {
    await page.click('#bar [data-menu="import"]'); await sleep(100); await page.click('.menu [data-id="import"]'); await sleep(400); includes(await text('#bar .plan'), 'Schwarzwald');
    await type('via Hotel Krone St. Peter'); includes(await text('#body'), '312 km→314.3 km'); assert(await has('.rt-ghost'), 'ghost'); await page.click('#pin [data-act="apply"]'); await sleep(300); assert((await S('App.S.mut().visits.length')) === 1, 'visit');
    await page.click('#body [data-act="undo"]'); await sleep(200); assert((await S('App.S.mut().visits.length')) === 0, 'undone');
    const p = await mapPoint({ map: 'bf', x: 510, y: 193 }); await page.mouse.click(p.x, p.y); await sleep(250); includes(await text('#callout'), 'not re-routed');
  });
  await step('back to the Alps via the plans page: wheel zoom switches to d4 and back; pins show when zoomed', async () => {
    await page.click('#bar .back'); await sleep(200); await openPlan('alps');
    const p = await mapPoint({ map: 'alps', x: 290, y: 378 }); await page.mouse.move(p.x, p.y); for (let i = 0; i < 6; i++) { await page.mouse.wheel({ deltaY: -300 }); await sleep(30); } await sleep(200);
    assert((await S('MapView.view.map')) === 'd4', 'd4'); assert(await has('.mk.pin'), 'pins'); for (let i = 0; i < 8; i++) { await page.mouse.wheel({ deltaY: 300 }); await sleep(30); } await sleep(200); assert((await S('MapView.view.map')) === 'alps', 'alps'); assert(!(await has('.mk.pin')), 'no pins at the whole trip');
  });
  await step('theme toggle in the site header flips the theme and back', async () => { const t0 = await S('document.documentElement.dataset.theme'); await page.click('#site .theme'); await sleep(100); const t1 = await S('document.documentElement.dataset.theme'); assert(t1 !== t0 && ['light', 'dark'].includes(t1), `${t0} → ${t1}`); await page.click('#site .theme'); await sleep(100); assert((await S('document.documentElement.dataset.theme')) === t0, 'back'); return `${t0} → ${t1} → ${t0}`; });
  await step('proto → Phone view (390 wide) → Rotate → landscape profile', async () => { await page.click('#bar .proto'); await sleep(100); await page.click('.menu [data-act="toggleFrame"]'); await sleep(500); assert(await has('#app.phone'), 'phone'); assert((await ev(() => document.getElementById('app').offsetWidth)) === 390, 'width'); await page.click('#frame-tools [data-act="rotate"]'); await sleep(500); assert(await has('#app.land'), 'land'); assert((await count('#land .pf-band')) === 8, 'handles'); });
  await page.close();
}

async function phone() {
  page = await browser.newPage();
  page.on('pageerror', (e) => results.push([false, 'phone pageerror', e.message]));
  page.on('console', (m) => { if (m.type() === 'error') results.push([false, 'phone console.error', m.text()]); });
  await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
  await page.goto(URL, { waitUntil: 'load' }); await sleep(500);
  const drag = async (x0, y0, x1, y1) => { const t = await page.touchscreen.touchStart(x0, y0); for (let i = 1; i <= 8; i++) await t.move(x0 + (x1 - x0) * i / 8, y0 + (y1 - y0) * i / 8); await t.end(); await sleep(500); };
  await step('phone: plans page with the rust band; the Alps row opens the planner', async () => { assert(await visible('#plans .band'), 'band'); includes(await text('#plans .band'), 'TRAILHEAD'); assert((await count('#plans .prow2')) === 3, 'rows'); await openPlan('alps', true); assert(!(await visible('#plans')), 'planner'); includes(await text('#nav .title'), 'Alps'); });
  await step('phone: collapsed by default: the head with totals and a search button, the profile, no box, no list', async () => { assert((await S('Sheet.detent')) === 'collapsed', 'collapsed'); assert(await visible('#pfslot .pf-plot'), 'profile in the sheet'); assert(!(await visible('#qin')), 'no box'); assert(!(await visible('#body')), 'no list'); includes(await text('#head'), '634 km'); assert(await visible('#head .sbtn'), 'search button'); const h = await S('Sheet.height'); assert(h > 200 && h < 340, 'height ' + h); return `collapsed ${Math.round(h)} · middle ${await S('Sheet.heights.middle')}`; });
  await step('phone: drag the head up → middle: profile, box and the days list; the list scrolls without a resize', async () => {
    const g = await rect('#head'); await drag(120, g.y, 120, g.y - 300); assert((await S('Sheet.detent')) === 'middle', 'middle'); assert(await visible('#qin'), 'box'); assert((await count('.day')) === 10, 'days'); assert(await visible('#pfslot .pf-plot'), 'profile stays');
    const h0 = await S('Sheet.height'); const b = await rect('#body'); await drag(120, b.y + b.h / 2 - 20, 120, b.y - b.h / 2 + 20); assert((await S('Sheet.detent')) === 'middle' && (await S('Sheet.height')) === h0, 'sheet unchanged'); const st = await ev(() => document.getElementById('body').scrollTop); assert(st > 0, 'list scrolled ' + st);
    const g2 = await rect('#head'); await drag(120, g2.y, 120, g2.y + 300); assert((await S('Sheet.detent')) === 'collapsed', 'collapsed again');
  });
  await step('phone: a horizontal drag on a profile handle moves the day end; a vertical drag on the profile moves the sheet', async () => {
    const b = await rect('#pfslot .pf-band[data-i="3"]'); await drag(b.x, b.y, b.x - 60, b.y + 2); const e = await S('App.S.mut().ends[3]'); assert(e < 316, 'moved ' + e); assert((await S('Sheet.detent')) === 'collapsed', 'sheet stayed');
    const b2 = await rect('#pfslot .pf-band[data-i="2"]'); await drag(b2.x, b2.y, b2.x + 3, b2.y - 250); assert((await S('Sheet.detent')) === 'middle', 'sheet took the vertical drag'); assert((await S('App.S.mut().ends[2]')) === 213, 'end 3 untouched'); await ev(() => { App.A.undo(); App.render(); Sheet.setDetent('collapsed'); }); await sleep(500);
  });
  await step('phone: search button → full screen; type → live list; Search → middle with the map, ticks and the action row', async () => {
    await page.tap('#head .sbtn'); await sleep(300); assert(await S('Sheet.full'), 'full'); assert(!(await visible('#pfslot')), 'no profile while typing'); assert(!(await ev(() => document.getElementById('qcancel').hidden)), 'Cancel'); await page.type('#qin', 'campsites end of day 4'); await sleep(300); assert((await count('.row.prow')) >= 3, 'live rows');
    await page.keyboard.press('Enter'); await sleep(600); assert(!(await S('Sheet.full')) && (await S('Sheet.detent')) === 'middle', 'middle'); includes(await text('#pin'), 'End Day 4 here'); assert(await has('.mk.res.sel'), 'marker'); assert(await has('#pfslot .pf-tick.sel'), 'tick on the profile'); assert(!(await has('.mk.dend[data-n="4"] .ml')), 'day-end label hidden near the selected place'); includes(await text('#head'), 'campsites');
  });
  await step('phone: picker grows the sheet; 10 km; tap a marker selects', async () => { const h0 = await S('Sheet.height'); await page.tap('[data-chip="within"]'); await sleep(400); assert((await S('Sheet.height')) > h0 + 40, 'grew'); await page.tap('.pick [data-km="10"]'); await sleep(200); includes(await text('#chips'), 'within 10 km'); await page.tap('.mk.res:not(.sel) .ring'); await sleep(200); assert(await has('.row.prow.sel'), 'sel'); });
  await step('phone: End Day 4 here → Out and back callout → applied; undo in the nav', async () => { await page.tap('.row.prow[data-id="camping-caravaneige-valloire"]'); await sleep(100); await page.tap('#pin [data-act="endDay"]'); await sleep(300); assert(await has('#callout [data-via="outback"]'), 'asks'); await page.tap('#callout [data-via="outback"]'); await sleep(400); includes(await text('#body'), 'Day 4 now ends at'); await page.tap('#nav [data-act="undo"]'); await sleep(200); assert((await S('App.S.hist.length')) === 0, 'undone'); });
  await step('phone: clear; tap a day end on the map → where chip and the Find row; ··· menu', async () => { await ev(() => { App.A.clear(); App.render(); Sheet.setDetent('collapsed'); }); await sleep(400); await page.tap('.mk.dend[data-n="3"] .ring'); await sleep(300); assert((await S('Sheet.detent')) === 'middle', 'middle'); includes(await text('#pointed'), 'End of Day 3'); includes(await text('#chips'), 'Find'); await ev(() => { App.A.clear(); App.render(); }); await page.tap('#nav [data-menu="more"]'); await sleep(150); includes(await text('.menu'), 'Save and send to device'); includes(await text('.menu'), 'Prototype · example data'); await page.touchscreen.tap(195, 800); await sleep(200); assert(!(await has('.menu')), 'menu closed'); });
  await step('phone: days list → long press → day menu; chevron → points; leg tap on the map → callout', async () => {
    await ev(() => Sheet.setDetent('middle')); await sleep(400); const r = await rect('.day[data-n="4"]'); const t = await page.touchscreen.touchStart(r.x, r.y); await sleep(650); await t.end(); await sleep(200); includes((await text('.menu')) + ' menu=' + (await S('App.S.menu')), 'Split this day'); await page.touchscreen.tap(195, 800); await sleep(200);
    await page.tap('.day[data-n="4"] .dchev'); await sleep(200); assert((await count('.dpts .ptrow')) === 4, 'points'); await ev(() => Sheet.setDetent('collapsed')); await sleep(400);
    await ev(() => { App.A.selDay({ n: 4 }); App.render(); }); await sleep(700); const p = await mapPoint({ map: 'd4', x: 560, y: 410 }); await page.touchscreen.tap(p.x, p.y); await sleep(300); includes(await text('#callout'), 'Routed');
  });
  await step('phone: Back → plans page → New route → tap map → Add point → building works with touch', async () => { await ev(() => { App.S.callout = null; App.render(); }); await page.tap('#nav [data-act="plans"]'); await sleep(300); assert(await visible('#plans'), 'plans'); await page.tap('#plans .new'); await sleep(500); includes(await text('#nav .title'), 'New route'); const p = await mapPoint({ map: 'bf', x: 500, y: 400 }); await page.touchscreen.tap(p.x, p.y); await sleep(300); includes(await text('#callout'), 'Add point'); await page.tap('#callout [data-act="coAdd"]'); await sleep(300); assert((await S('App.S.mut().legs.length')) === 1, 'leg'); assert(await has('#pfslot .pf-plot'), 'profile of the built route'); });
  await page.close();
  page = await browser.newPage();
  await page.setViewport({ width: 844, height: 390, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
  await page.goto(URL, { waitUntil: 'load' }); await sleep(500);
  await step('landscape: the profile alone with 8 handles; a drag moves an end', async () => { await openPlan('alps', true); assert(await has('#app.land'), 'land'); assert((await count('#land .pf-band')) === 8, 'handles'); const b = await rect('#land .pf-band[data-i="3"]'); await drag(b.x, b.y, b.x - 40, b.y); assert((await S('App.S.mut().ends[3]')) < 316, 'moved'); });
  await page.close();
}

let browser;
(async () => {
  browser = await puppeteer.launch({ executablePath: CHROME, headless: true });
  await web(); await phone();
  await browser.close();
  for (const [ok, name, info] of results) console.log((ok ? 'PASS ' : 'FAIL ') + name + (info ? '  — ' + info : ''));
  console.log(`${results.filter((r) => r[0]).length}/${results.length} passed`);
})();
