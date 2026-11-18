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
const W = Number(process.env.W || 1180), H = Number(process.env.H || 900);
async function run(mode, name, before, video) {
  const { browser, page } = await open({ params: `?small=${mode}`, cover: true, width: W, height: H, video });
  await page.getByRole('button', { name: 'Places ▾' }).click();
  await page.getByRole('button', { name: 'Food', exact: true }).click();
  await page.locator('.places .place').first().waitFor({ timeout: 60000 });
  await page.waitForTimeout(3000);
  const buttons = page.locator('.places .place > button');
  const counts = await buttons.evaluateAll(bs => bs.map(b => b.querySelector('.count') ? 0 : b.getAttribute('aria-label') !== 'restaurant' ? 1 : 0));
  console.log('labels', await buttons.evaluateAll(bs => bs.filter(b => !b.querySelector('.count')).map(b => b.getAttribute('aria-label')).join(' | ')));
  const box = await buttons.nth(counts.indexOf(1)).boundingBox();
  const [x, y] = [box.x + 11, box.y + box.height / 2];
  if (before) {
    await page.mouse.move(W - 40, 120);
    await page.screenshot({ path: OUT + '2-before.png' });
  }
  if (video) { await page.mouse.move(x - 200, y - 60); await page.waitForTimeout(600); await page.mouse.move(x, y, { steps: 12 }); await page.waitForTimeout(300); await cursor(page, x, y); }
  await page.mouse.click(x, y);
  await cursor(page, x, y);
  await page.waitForTimeout(video ? 2500 : 4000);
  if (!video) await page.screenshot({ path: OUT + name + '.png' });
  await page.close();
  await browser.close();
}
(async () => {
  if (process.argv[2] === 'video') return run('swap', '2A-swap', false, true);
  await run('stay', '2B-stay', true);
  await run('swap', '2A-swap', false);
})();
