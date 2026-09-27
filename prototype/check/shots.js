// One round of screenshots: node check/shots.js  → check/out/*.png, console errors printed.
const puppeteer = require('puppeteer-core');
const path = require('path');
const fs = require('fs');
const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const URL = 'file://' + path.resolve(__dirname, '..', 'index.html');
const OUT = path.join(__dirname, 'out');
fs.mkdirSync(OUT, { recursive: true });

(async () => {
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: true, args: ['--hide-scrollbars'] });
  const errors = [];
  async function shoot(name, { w, h, mobile, dark, before }) {
    const page = await browser.newPage();
    page.on('console', (m) => { if (m.type() === 'error' || m.type() === 'warning') errors.push(`${name}: console.${m.type()}: ${m.text()}`); });
    page.on('pageerror', (e) => errors.push(`${name}: pageerror: ${e.message}`));
    await page.emulateMediaFeatures([{ name: 'prefers-color-scheme', value: dark ? 'dark' : 'light' }]);
    await page.setViewport({ width: w, height: h, deviceScaleFactor: 2, isMobile: !!mobile, hasTouch: !!mobile });
    await page.goto(URL, { waitUntil: 'load' });
    await new Promise((r) => setTimeout(r, 400));
    if (before) await before(page);
    await new Promise((r) => setTimeout(r, 600));
    await page.screenshot({ path: path.join(OUT, name + '.png') });
    await page.close();
    console.log('shot', name);
  }
  const typeTent = async (page) => { await page.click('#qin'); await page.type('#qin', 'campsites end of day 4'); await page.keyboard.press('Enter'); await new Promise((r) => setTimeout(r, 700)); await page.click('[data-chip="within"]'); };
  await shoot('web-light-rest', { w: 1440, h: 900 });
  await shoot('web-dark-rest', { w: 1440, h: 900, dark: true });
  await shoot('web-light-tent', { w: 1440, h: 900, before: typeTent });
  await shoot('phone-light-rest', { w: 390, h: 844, mobile: true });
  await shoot('phone-dark-rest', { w: 390, h: 844, mobile: true, dark: true, before: async (page) => { await page.tap('#nav [data-menu="more"]'); } });
  await browser.close();
  console.log(errors.length ? 'ERRORS:\n' + errors.join('\n') : 'no console errors');
})();
