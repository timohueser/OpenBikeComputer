// One scripted run through the acceptance list: node check/run.js [url]
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
const S = (expr) => ev((e) => eval(e), expr);
async function type(t, enter = true) { await page.click('#qclear').catch(() => {}); await page.click('#qin'); await ev(() => { App.A.clear(); App.render(); }); await page.type('#qin', t, { delay: 5 }); if (enter) await page.keyboard.press('Enter'); await sleep(200); }
const assert = (c, m) => { if (!c) throw new Error('assert: ' + m); };
const includes = (a, b) => { assert(String(a).includes(b), `"${String(a).slice(0, 90)}" does not include "${b}"`); };

async function web() {
  page = await browser.newPage();
  page.on('pageerror', (e) => results.push([false, 'pageerror', e.message]));
  page.on('console', (m) => { if (m.type() === 'error') results.push([false, 'console.error', m.text()]); });
  await page.setViewport({ width: 1440, height: 900 });
  await page.goto(URL, { waitUntil: 'load' }); await sleep(500);

  await step('rest: days list with bars', async () => { assert((await count('.day')) === 10, 'days'); assert((await count('.day .bar')) === 9, 'bars'); });
  await step('select Day 4: list, map (d4), profile', async () => { await page.click('.day[data-n="4"]'); await sleep(600); assert(await has('.day.sel[data-n="4"]'), 'list'); assert((await S('MapView.view.map')) === 'd4', 'map d4'); assert(await has('.day-band.sel'), 'profile band'); assert(await has('#prof .ph .h'), 'profile head'); return await text('#prof .ph .h'); });
  await step('select Day 2: map back to alps, framed', async () => { await page.click('.day[data-n="2"]'); await sleep(600); assert((await S('MapView.view.map')) === 'alps', 'alps'); return 'scale ' + (await S('MapView.view.scale.toFixed(2)')); });
  await step('profile day end → where chip + Find row', async () => { await page.click('#prof .dend[data-n="4"]'); await sleep(200); includes(await text('#pointed'), 'End of Day 4'); includes(await text('#chips'), 'Find'); });
  await step('type after the chip: "campsites" → list at End of Day 4', async () => { await page.type('#qin', 'campsites'); await sleep(200); includes(await text('#chips'), 'End of Day 4'); assert((await S('App.S.ans.kind')) === 'places', 'places'); assert((await S('App.S.ans.where.type')) === 'day', 'where day'); });
  await step('Find row: pointed chip + Water', async () => { await ev(() => { App.A.clear(); App.render(); }); await page.click('#prof .dend[data-n="4"]'); await sleep(100); await page.click('#chips [data-act="find"][data-k="water"]'); await sleep(200); includes(await text('#chips'), 'Water'); return await text('#oneline'); });
  await step('map day end tap → where chip', async () => { await ev(() => { App.A.clear(); App.render(); }); await page.click('.mk.dend[data-n="3"] .ring'); await sleep(200); includes(await text('#pointed'), 'End of Day 3'); await ev(() => { App.A.clear(); App.render(); }); });
  await step('"campsites end of day 4": chips, sections, selection, marks, tick', async () => {
    await type('campsites end of day 4');
    const chips = await text('#chips'); includes(chips, 'Campsites'); includes(chips, 'End of Day 4'); includes(chips, 'within 5 km');
    assert((await count('.row.prow')) === 5, 'rows ' + (await count('.row.prow'))); includes(await text('#oneline'), '2 within 5 km · 3 earlier');
    includes(await text('.row.prow.sel'), 'Caravaneige'); assert(await has('.mk.res.sel'), 'map sel'); assert(await has('#prof .tick.sel'), 'tick');
    includes(await text('#pin'), 'End Day 4 here'); includes(await text('#pin'), 'Add as stop');
  });
  await step('within picker → 25 km: 4 in the ring, sentence faint', async () => { await page.click('[data-chip="within"]'); await sleep(100); includes(await text('.pick'), '2 km'); await page.click('.pick [data-km="25"]'); await sleep(200); includes(await text('#chips'), 'within 25 km'); includes(await text('#oneline'), '3 within 25 km'); assert(await has('.qbox.edited'), 'faint'); });
  await step('kinds picker → + Lodging', async () => { await page.click('[data-chip="what"]'); await sleep(100); await page.click('.pick [data-act="pickKind"][data-k="lodging"]'); await sleep(200); includes(await text('#chips'), 'Campsites and lodging'); includes(await text('#body'), 'Christiania'); await page.click('[data-chip="what"]'); });
  await step('where picker → Day 5 end, then This map view', async () => { await page.click('[data-chip="where"]'); await sleep(100); await page.click('.pick [data-act="pickWhere"][data-n="5"]'); await sleep(300); includes(await text('#chips'), 'Day 5'); await page.click('.pick [data-type="view"]'); await sleep(300); includes(await text('#chips'), 'In this map view'); return await text('#oneline'); });
  await step('"shops open sunday day 4": filter, dimmed last, Any day → struck, restore', async () => {
    await type('shops open sunday day 4'); includes(await text('#chips'), 'Open on Sunday'); includes(await text('#oneline'), '5 shops · 3 open on Sunday');
    const last = await ev(() => [...document.querySelectorAll('.row.prow')].pop().textContent); includes(last, 'Intermarché'); assert(await has('.row.prow.dim'), 'dim');
    await page.click('[data-chip="open"]'); await sleep(100); await page.click('.pick [data-wd="any"]'); await sleep(200); assert(await has('.chip.off'), 'struck'); includes(await text('.chip.off'), 'Open on Sunday');
    await page.click('.chip.off'); await sleep(200); includes(await text('#chips'), 'Open on Sunday'); assert(!(await has('.chip.off')), 'restored');
  });
  await step('dates off → "shops open day 4" needs a date → Set trip dates', async () => {
    await page.click('#bar .dates'); await sleep(100); await page.click('.menu [data-act="toggleDates"]'); await sleep(100); await page.click('#menus .scrim').catch(() => {}); await sleep(100);
    await type('shops open day 4'); assert(await has('.chip.need'), 'need chip'); includes(await text('#body'), 'Day 4 has no date'); assert((await count('.row.prow')) === 5, 'list still shown');
    await page.click('#body [data-act="setDates"]'); await sleep(200); includes(await text('#chips'), 'Open on Sun 20 Jun'); assert(!(await has('.chip.need')), 'need gone');
  });
  await step('"shops open sunday day 4" works without dates', async () => { await ev(() => { App.S.dates = false; }); await type('shops open sunday day 4'); includes(await text('#oneline'), '3 open on Sunday'); await ev(() => { App.S.dates = true; }); });
  await step('"bike shop end of day 4": empty, Search within 25 km, nearest', async () => { await type('bike shop end of day 4'); includes(await text('#body'), 'No bike shops within 5 km of the end of Day 4.'); includes(await text('#body'), 'Nearest on the route'); includes(await text('#body'), 'Cycles Maurienne'); await page.click('[data-act="wider"]'); await sleep(200); includes(await text('#chips'), 'within 25 km'); assert((await count('.row.prow')) === 1, 'one row'); });
  await step('"campsites with a pool end of day 4": partly understood', async () => { await type('campsites with a pool end of day 4'); includes(await text('.chip.off'), 'with a pool'); assert((await count('.row.prow')) === 5, 'rows'); });
  await step('"is the galibier worth it": not understood + place', async () => { await type('is the galibier worth it'); includes(await text('#body'), 'Not understood'); includes(await text('#body'), 'Col du Galibier'); assert((await count('.chip.off')) === 2, 'two off chips'); });
  await step('"water gaps": stretch, Add marker, Undo', async () => { await type('water gaps'); includes(await text('#body'), 'No mapped water for 29 km'); includes(await text('#pin'), 'Add marker'); await page.click('#pin [data-act="addMarker"]'); await sleep(200); includes(await text('#body'), "Marker added at Val d'Isère"); assert((await S('App.S.mut().points.length')) === 1, 'point'); await page.click('#body [data-act="undo"]'); await sleep(200); assert((await S('App.S.mut().points.length')) === 0, 'undone'); });
  await step('"end day 4 at saint-michel": proposal, ghost marks, Apply, days update', async () => {
    await type('end day 4 at saint-michel'); const b = await text('#body'); includes(b, '104 km→88 km'); includes(b, '5 h 20'); includes(b, '69 km'); includes(b, 'Col du Télégraphe'); includes(b, 'The line does not change.');
    assert(await has('.mk.newEnd'), 'ghost'); assert(await has('#prof .dend.new'), 'profile new end');
    await page.click('#pin [data-act="apply"]'); await sleep(300); includes(await text('#body'), 'Day 4 now ends at Saint-Michel-de-Maurienne'); includes(await text('.day[data-n="4"]'), '88 km'); includes(await text('.day[data-n="5"]'), '69 km'); includes(await text('.day[data-n="5"] .t'), 'Saint-Michel');
  });
  await step('Undo / Redo via the app bar', async () => { await page.click('#bar [data-act="undo"]'); await sleep(200); includes(await text('.day[data-n="4"]'), '104 km'); await page.click('#bar [data-act="redo"]'); await sleep(200); includes(await text('.day[data-n="4"]'), '88 km'); await page.click('#bar [data-act="undo"]'); await sleep(200); includes(await text('.day[data-n="4"]'), '104 km'); });
  await step('list → select Le Petit Nice → End Day 4 here → 87 km, Undo', async () => { await type('campsites end of day 4'); await page.click('.row.prow[data-id="camping-le-petit-nice-saint-michel-de-maurienne"]'); await sleep(200); await page.click('#pin [data-act="endDay"]'); await sleep(300); includes(await text('.day[data-n="4"]'), '87 km'); includes(await text('#body'), 'Day 4 now ends at'); assert(await has('.mk.dend[data-n="4"]'), 'map end'); await page.click('#body [data-act="undo"]'); await sleep(200); includes(await text('.day[data-n="4"]'), '104 km'); });
  await step('Add as stop → point, Undo', async () => { await type('campsites end of day 4'); await page.click('#pin [data-act="addStop"]'); await sleep(200); includes(await text('#body'), 'added as a stop'); assert(await has('.mk.pt'), 'point on map'); await page.click('#body [data-act="undo"]'); await sleep(200); assert(!(await has('.mk.pt')), 'undone'); });
  await step('parser variations', async () => {
    const out = [];
    for (const [t, want] of [['day 4 campsites', 'places:day:whole'], ['Zeltplatz Ende Tag 4', 'places:day:end'], ['Supermarkt Mitte Tag 4', 'places:day:middle'], ['campsits end of day 4', 'places:day:end'], ['shops within 10 km end of day 4', 'within10'], ['tomorrow campsites', 'places:day:4'], ['end day 4 at a campsite', 'endcamp'], ['pharmacies along route', 'places:route'], ['shops middle of day 4', 'places:day:middle'], ['shops', 'places:view']]) {
      await type(t, false); const r = await S('(function(){const r=App.S.req,w=r.where||{};return [r.kind,w.type,w.part,w.day,r.within,(App.S.ans&&App.S.ans.actions&&App.S.ans.actions.primary.label)||""].join(":")})()');
      const ok = want === 'within10' ? r.includes(':10:') : want === 'endcamp' ? r.startsWith('places:day:end') && r.endsWith('End Day 4 here') : want === 'places:day:4' ? r.startsWith('places:day:whole:4') : r.includes(want); out.push(`${t} → ${r}`); assert(ok, `${t} → ${r}`);
    }
    return out.join(' | ');
  });
  await step('plan switch → New route; "Titisee" → 3 options, select, Send', async () => {
    await page.click('#bar .plan'); await sleep(100); await page.click('.menu [data-id="new"]'); await sleep(400); assert((await S('MapView.view.map')) === 'bf', 'bf');
    await type('Titisee'); assert((await S('App.S.ans.kind')) === 'route', 'route'); assert((await count('.row.orow')) === 3, 'options'); includes(await text('#chips'), 'From here'); includes(await text('#chips'), 'Glottertal');
    await page.click('.row.orow[data-id="leastclimb"]'); await sleep(200); includes(await text('.row.orow.sel'), 'Least climbing'); includes(await text('#prof'), 'No profile');
    await page.click('#pin [data-act="send"]'); await sleep(200); includes(await text('#body'), 'Sent to OBC'); assert((await S('App.S.mut().route.opt')) === 'leastclimb', 'route used');
  });
  await step('"Kandel" → one place, others, Route from here → 20.8 km', async () => { await type('Kandel'); assert((await S('App.S.ans.kind')) === 'place', 'place'); includes(await text('#body'), 'Other places with this name'); includes(await text('#body'), 'Rhineland-Palatinate'); await page.click('#pin [data-act="routeFrom"]'); await sleep(300); includes(await text('#body'), '20.8 km'); includes(await text('#body'), 'No other way is shorter'); includes(await text('#chips'), 'Denzlingen'); });
  await step('"road bike route up Kandel starting here" → Road route; bike picker', async () => { await type('road bike route up Kandel starting here'); includes(await text('#chips'), 'Road'); includes(await text('#body'), '1,020 m'); await page.click('[data-chip="bike"]'); await sleep(100); await page.click('.pick [data-bike="gravel"]'); await sleep(200); includes(await text('#chips'), 'Gravel'); });
  await step('plan import → "via Hotel Krone St. Peter" → proposal, ghost, Apply, Undo', async () => {
    await page.click('#bar .plan'); await sleep(100); await page.click('.menu [data-id="import"]'); await sleep(400);
    await type('via Hotel Krone St. Peter'); includes(await text('#chips'), 'Add a visit'); includes(await text('#body'), '312 km→314.3 km'); assert(await has('.rt-ghost'), 'ghost line');
    await page.click('#pin [data-act="apply"]'); await sleep(300); includes(await text('#body'), 'added as a visit'); assert(!(await has('.rt-ghost')), 'ghost gone'); assert(await has('.mk.pt'), 'visit point');
    await page.click('#body [data-act="undo"]'); await sleep(200); assert(!(await has('.mk.pt')), 'undone');
  });
  await step('back to Alps: wheel zoom switches to d4, zoom out back to alps, drag pans', async () => {
    await page.click('#bar .plan'); await sleep(100); await page.click('.menu [data-id="alps"]'); await sleep(500);
    const p = await ev(() => { const c = MapView.convert({ map: 'alps', x: 290, y: 378 }, MapView.view.map), r = document.getElementById('map').getBoundingClientRect(), v = MapView.view; return { x: r.left + r.width / 2 + (c.x - v.cx) * v.scale, y: r.top + r.height / 2 + (c.y - v.cy) * v.scale }; });
    await page.mouse.move(p.x, p.y); for (let i = 0; i < 6; i++) { await page.mouse.wheel({ deltaY: -300 }); await sleep(30); } await sleep(200);
    assert((await S('MapView.view.map')) === 'd4', 'd4 after zoom, scale ' + (await S('MapView.view.scale')));
    for (let i = 0; i < 8; i++) { await page.mouse.wheel({ deltaY: 300 }); await sleep(30); } await sleep(200); assert((await S('MapView.view.map')) === 'alps', 'alps after zoom out');
    const cx = await S('MapView.view.cx'); await page.mouse.move(900, 400); await page.mouse.down(); await page.mouse.move(700, 450, { steps: 8 }); await page.mouse.up(); await sleep(100); assert((await S('MapView.view.cx')) > cx, 'panned');
    await page.click('[data-act="zoomIn"]'); await sleep(500); await page.click('[data-act="fitAll"]'); await sleep(500);
  });
  await step('menus: versions, offline, import, qr, proto → Phone view', async () => { for (const m of ['versions', 'offline', 'qr']) { await page.click(`#bar [data-menu="${m}"]`); await sleep(100); assert(await has('.menu'), m); await page.click('#menus .scrim'); await sleep(50); } await page.click('#bar .proto'); await sleep(100); await page.click('.menu [data-act="toggleFrame"]'); await sleep(500); assert(await has('#app.phone.framed'), 'framed'); assert((await ev(() => document.getElementById('app').offsetWidth)) === 390, 'width 390'); });
  await page.close();
}

async function phone() {
  page = await browser.newPage();
  page.on('pageerror', (e) => results.push([false, 'phone pageerror', e.message]));
  page.on('console', (m) => { if (m.type() === 'error') results.push([false, 'phone console.error', m.text()]); });
  await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true, deviceScaleFactor: 2 });
  await page.goto(URL, { waitUntil: 'load' }); await sleep(500);
  const drag = async (x, y0, y1) => { const t = await page.touchscreen.touchStart(x, y0); for (let i = 1; i <= 6; i++) await t.move(x, y0 + (y1 - y0) * i / 6); await t.end(); await sleep(500); };
  await step('phone: layout, sheet at medium, days with 44pt+ rows', async () => { assert(await has('#app.phone'), 'phone'); assert((await S('Sheet.detent')) === 'medium', 'medium'); const h = await ev(() => document.querySelector('.day').offsetHeight); assert(h >= 56, 'row ' + h); includes(await text('#pin'), 'Send to device'); });
  await step('phone: drag handle up → high; down twice → low; heights', async () => {
    const gy = await ev(() => document.querySelector('#panel .grab').getBoundingClientRect().top);
    await drag(195, gy, gy - 300); assert((await S('Sheet.detent')) === 'high', 'high'); const gy2 = await ev(() => document.querySelector('#panel .grab').getBoundingClientRect().top);
    await drag(195, gy2, gy2 + 250); assert((await S('Sheet.detent')) === 'medium', 'medium again'); const gy3 = await ev(() => document.querySelector('#panel .grab').getBoundingClientRect().top);
    await drag(195, gy3, gy3 + 300); assert((await S('Sheet.detent')) === 'low', 'low'); return JSON.stringify(await S('Sheet.heights'));
  });
  await step('phone: type → answer raises the sheet, pin bar, chip sizes', async () => { await page.click('#qin'); await page.type('#qin', 'campsites end of day 4'); await page.keyboard.press('Enter'); await sleep(400); assert((await S('Sheet.detent')) === 'medium', 'raised'); includes(await text('#pin'), 'End Day 4 here'); const ch = await ev(() => document.querySelector('#chips .chip').offsetHeight); assert(ch >= 36, 'chip ' + ch); const bh = await ev(() => document.querySelector('#pin .btn.primary').offsetHeight); assert(bh === 50, 'button ' + bh); });
  await step('phone: Earlier on Day 4 row expands', async () => { assert((await count('.row.prow')) === 3, 'collapsed rows ' + (await count('.row.prow'))); await page.tap('.row.more'); await sleep(200); assert((await count('.row.prow')) === 6, 'expanded'); });
  await step('phone: picker inline (within → 10 km)', async () => { await page.tap('[data-chip="within"]'); await sleep(150); assert(await has('.pick'), 'pick'); await page.tap('.pick [data-km="10"]'); await sleep(200); includes(await text('#chips'), 'within 10 km'); });
  await step('phone: tap a result marker → selects the row', async () => { await page.tap('.mk.res:not(.sel) .ring'); await sleep(200); assert(await has('.row.prow.sel'), 'sel'); });
  await step('phone: clear, tap a day end on the map → where chip', async () => { await page.tap('#qclear'); await sleep(200); await page.tap('.mk.dend[data-n="4"] .ring'); await sleep(300); includes(await text('#pointed'), 'End of Day 4'); includes(await text('#chips'), 'Find'); });
  await step('phone: ··· menu, plan menu', async () => { await page.tap('#nav [data-menu="more"]'); await sleep(150); includes(await text('.menu'), 'Prototype · example data'); includes(await text('.menu'), 'Trip dates'); await page.tap('#menus .scrim'); await sleep(100); await page.tap('#nav .title'); await sleep(150); includes(await text('.menu'), 'New route from Glottertal'); await page.tap('#menus .scrim'); });
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
