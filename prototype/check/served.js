// Load the served page once and report console errors: node check/served.js http://localhost:8765/
const puppeteer = require('puppeteer-core');
(async () => {
  const b = await puppeteer.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true });
  const errors = [];
  for (const [w, h, mobile] of [[1440, 900, false], [390, 844, true]]) {
    const p = await b.newPage();
    p.on('console', (m) => { if (m.type() === 'error' || m.type() === 'warning') errors.push(m.text()); });
    p.on('pageerror', (e) => errors.push(e.message));
    p.on('requestfailed', (r) => errors.push('request failed: ' + r.url()));
    await p.setViewport({ width: w, height: h, isMobile: mobile, hasTouch: mobile });
    await p.goto(process.argv[2], { waitUntil: 'networkidle0' });
    await new Promise((r) => setTimeout(r, 500));
    console.log(w + 'x' + h, 'loaded, title:', await p.title(), 'days:', await p.evaluate(() => document.querySelectorAll('.day').length));
    await p.close();
  }
  await b.close();
  console.log(errors.length ? 'ERRORS:\n' + errors.join('\n') : 'served: no console errors');
})();
