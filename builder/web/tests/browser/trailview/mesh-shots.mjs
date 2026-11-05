// SCRATCH (question 4): pictures of the near-mesh page (`debug heights --mesh`, q4w/mesh/index.html).
//   node mesh-shots.mjs PAGE OUT_DIR NAME=place,mode,cam ...
import { launch } from './harness.js';
const [page, out, ...shots] = process.argv.slice(2);
const browser = await launch(true);
const tab = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
for (const s of shots) {
    const [name, spec] = s.split('=');
    const [place, mode, ...cam] = spec.split(',');
    await tab.goto(`file://${page}?shot=${name}#place=${place}&mode=${mode}&cam=${cam.join(',')}`);
    await tab.waitForFunction(() => window.__ready === true, null, { timeout: 60000 });
    await tab.waitForTimeout(800);
    await tab.screenshot({ path: `${out}/${name}.png` });
    console.log(name);
}
await browser.close();
