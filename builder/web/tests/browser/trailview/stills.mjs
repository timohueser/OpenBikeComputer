// The parts that the scripts of TrailView stills share: the page that renders stills, the production
// release, the bucket command and the check of the public host.

import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { routeFixture } from './harness.js';

export const HOST = 'https://maps.openbikecomputer.com', CATALOG = `${HOST}/planner/catalog.json`;
export const IMMUTABLE = 'public, max-age=31536000, immutable', LIVE = 'public, max-age=60, must-revalidate';
/** After a tile fails, a script waits this long and tries the still once more: the tile host answers 429 to too many requests. */
export const RETRY_MS = 60_000;
/** The longest still on SwiftShader, which draws for minutes; on a GPU the default of `still()` holds. */
export const SOFTWARE_MS = 600_000;

export const readJson = file => (existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : undefined);

/** A page with the still function; throws when the browser has no GPU for stills. */
export async function stillPage(browser, origin, { gpu, fixture }) {
    const page = await browser.newPage();
    if (fixture) await routeFixture(page);
    await page.goto(`${origin}/trailview.html?still`);
    await page.waitForFunction(() => window.trailviewStill || document.querySelector('.notice'), null, { timeout: 60_000 });
    if (!await page.evaluate(software => window.trailviewStill?.canRenderStills({ software }), !gpu)) throw new Error('This browser has no GPU for stills: run render with --gpu on a machine with a GPU.');
    return page;
}

/** The active release of the production catalogue: its `id`, `bounds` and `routes`. */
export async function productionRelease(fetch) {
    const response = await fetch(CATALOG);
    if (response.status !== 200) throw new Error(`${CATALOG}: ${response.status}.`);
    return JSON.parse(await response.text()).active;
}

/**
 * `obc data r2 ARGS`, which resolves to its standard output. It rejects when it fails, with the
 * standard error in `stderr`. Several can run at a time.
 */
export function obcR2(args) {
    return new Promise((resolve, reject) => {
        const child = spawn('obc', ['data', 'r2', ...args], { stdio: ['inherit', 'pipe', 'pipe'] });
        let stdout = '', stderr = '';
        child.stdout.setEncoding('utf8').on('data', data => stdout += data);
        child.stderr.setEncoding('utf8').on('data', data => {
            stderr += data;
            process.stderr.write(data);
        });
        child.on('error', reject);
        child.on('close', status => {
            if (status !== 0) return reject(Object.assign(new Error(`obc data r2 ${args.join(' ')} failed`), { stderr }));
            if (!args.includes('--json')) process.stdout.write(stdout);
            resolve(stdout);
        });
    });
}

/** Throws unless the public host answers 200 for each key; `what` names the keys in the message. */
export async function checkHost(keys, fetch, what) {
    const failed = [];
    for (const key of keys) {
        const { status } = await fetch(`${HOST}/${key}`, { method: 'HEAD' });
        if (status !== 200) failed.push(`${HOST}/${key}: ${status}`);
    }
    if (failed.length) throw new Error(`The public host does not serve ${what}:\n${failed.join('\n')}`);
}
