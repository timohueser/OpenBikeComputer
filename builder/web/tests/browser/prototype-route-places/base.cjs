const { chromium } = require('/opt/node-tools/node_modules/playwright');
const fs = require('fs');
const BASE = 'http://127.0.0.1:5316';
const record = JSON.parse(fs.readFileSync(__dirname + '/route-record.json', 'utf8'));
async function open({ params = '', cover = false, width = 1440, height = 1000, video = false } = {}) {
  const browser = await chromium.launch({ proxy: { server: process.env.HTTPS_PROXY, bypass: '127.0.0.1,localhost' } });
  const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: video ? 1 : 2, ...video && { recordVideo: { dir: __dirname + '/video', size: { width, height } } } });
  page.on('pageerror', e => console.log('pageerror', e.message));
  await page.goto(`${BASE}/planner.html?cover=seed`);
  await page.waitForFunction(() => window.coverTest, null, { timeout: 60000 });
  await page.evaluate(async ({ record, cover }) => {
    const { library, lineKey } = window.coverTest;
    const saved = await library.saveRoute(record);
    if (cover) {
      const image = await (await fetch(cover.url)).blob();
      await library.saveCover({ id: saved.id, lineKey: lineKey(saved), state: 'done', tries: 0, image, points: cover.points });
    }
  }, { record, cover: cover ? { url: '/src/lib/covers/testdata/cover.webp', points: JSON.parse(fs.readFileSync('/home/user/wt-route-places/builder/web/src/lib/covers/testdata/cover.json', 'utf8')).points } : null });
  await page.goto(`${BASE}/planner.html${params}#/route/${record.id}`);
  await page.locator('.lanes').waitFor({ timeout: 60000 });
  return { browser, page };
}
module.exports = { open };
if (require.main === module) (async () => {
  const { browser, page } = await open({ params: process.argv[2] || '' });
  await page.waitForTimeout(6000);
  await page.screenshot({ path: __dirname + '/base.png' });
  await browser.close();
})();
