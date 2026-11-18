const { open } = require('./base.cjs');
const OUT = (process.env.OUT || __dirname + '/out') + '/';
const cursor = (page, x, y) => page.evaluate(([x, y]) => {
  document.getElementById('proto-cursor')?.remove();
  const el = document.createElement('div');
  el.id = 'proto-cursor';
  el.style.cssText = `position:fixed;left:${x}px;top:${y}px;z-index:99999;pointer-events:none;width:22px;height:30px`;
  el.innerHTML = '<svg width="22" height="30" viewBox="0 0 22 30"><path d="M1 1 L1 23 L6.5 17.5 L10.5 27 L14 25.5 L10 16.5 L18 16.5 Z" fill="#111" stroke="#fff" stroke-width="1.6" stroke-linejoin="round"/></svg>';
  document.body.appendChild(el);
}, [x, y]);
async function shoot(mode, pick, name, zoom) {
  const { browser, page } = await open({ params: `?group=${mode}`, height: 1060 });
  await page.getByRole('button', { name: 'Places ▾' }).click();
  await page.getByRole('button', { name: 'Food', exact: true }).click();
  await page.locator('.places .place').first().waitFor({ timeout: 60000 });
  await page.waitForTimeout(2500);
  const buttons = page.locator('.places .place > button');
  const counts = await buttons.evaluateAll(bs => bs.map(b => Number(b.querySelector('.count')?.textContent ?? '1')));
  const index = counts.findIndex(pick);
  const box = await buttons.nth(index).boundingBox();
  const [x, y] = [box.x + 11, box.y + box.height / 2];
  await page.mouse.move(x, y);
  await page.waitForTimeout(400);
  await cursor(page, x, y);
  await page.screenshot({ path: OUT + name + '.png' });
  console.log(name, 'count', counts[index], 'discs', await page.locator('.proto-disc').count());
  if (zoom) {
    await page.evaluate(() => {
      const c = window.protoGroup, lons = c.map(p => p[0]), lats = c.map(p => p[1]);
      window.protoMap.fitBounds([[Math.min(...lons), Math.min(...lats)], [Math.max(...lons), Math.max(...lats)]], { padding: 120, maxZoom: 17, animate: false });
    });
    await page.waitForTimeout(5000);
    await page.screenshot({ path: OUT + name + '-zoom.png' });
    console.log('zoom', await page.evaluate(() => window.protoMap.getZoom()));
  }
  await browser.close();
}
(async () => {
  await shoot('all', c => c === 1, '1-single', false);
  await shoot('all', c => c >= 6 && c <= 15 && c === 10, '1A-group-all', true);
  await shoot('one', c => c === 10, '1B-group-one', true);
  await shoot('none', c => c === 10, '1C-group-none', true);
})();
