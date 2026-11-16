// The browser environment of a study: serves the built planner on loopback, starts a clean browser
// context for each walkthrough, takes stable screenshots and performs the participant's actions.
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';
import { loopbackPort } from '../../../../../tools/loopback_port.mjs';

const VIEWPORT = { width: 1280, height: 800 };
const STABLE_GAP_MS = 300;
const STABLE_LIMIT_MS = 5_000;
const SAME_POINT_PX = 20;
const SCROLL_PX = 400;
const BROWSER_DIR = fileURLToPath(new URL('..', import.meta.url));
const DIST = fileURLToPath(new URL('../../../dist/planner', import.meta.url));

const point = { anyOf: [{ type: 'integer' }, { type: 'null' }] };
const nullableText = { anyOf: [{ type: 'string' }, { type: 'null' }] };

/** The `action` part of the step record schema. */
const schema = {
  type: 'object',
  properties: {
    type: { type: 'string', enum: ['click', 'drag', 'scroll', 'hover', 'type', 'key', 'give_up'] },
    target: { type: 'string' },
    x: point, y: point, x2: point, y2: point,
    text: nullableText,
  },
  required: ['type', 'target', 'x', 'y', 'x2', 'y2', 'text'],
  additionalProperties: false,
};

const text = `The screenshot is ${VIEWPORT.width} × ${VIEWPORT.height} pixels. A point is x, y in pixels from the top left.
- click: click at x, y.
- drag: press at x, y, move to x2, y2, release.
- scroll: turn the mouse wheel at x, y; text is "up" or "down".
- hover: move the mouse to x, y.
- type: click at x, y, then type text.
- key: press one key, such as "Enter", "Escape" or "Tab"; text is the key name.
- give_up: stop trying.
Name the target in words. Use null for each field that the action does not use.`;

/** True when two actions are the same: the same type and text, and each point within 20 pixels. */
function same(a, b) {
  const near = (x1, y1, x2, y2) => (x1 === null && x2 === null) || Math.hypot(x1 - x2, y1 - y2) <= SAME_POINT_PX;
  return a.type === b.type && (a.text ?? null) === (b.text ?? null)
    && near(a.x, a.y, b.x, b.y) && near(a.x2, a.y2, b.x2, b.y2);
}

/** Serves the build and starts Chromium. `close` stops both. */
export async function openBrowser() {
  const port = await loopbackPort('OBC_UX_STUDY_PORT');
  const base = `http://127.0.0.1:${port}`;
  const server = spawn('python3', ['serve.py', String(port), DIST], { cwd: BROWSER_DIR, stdio: 'inherit' });
  // A cloud session reaches the public hosts only through its HTTPS proxy, and Chromium does not read the
  // proxy from the environment.
  const proxy = process.env.HTTPS_PROXY ? { server: process.env.HTTPS_PROXY, bypass: '127.0.0.1,localhost' } : undefined;
  const browser = await chromium.launch({ proxy });
  await waitForServer(`${base}/planner.html`);
  return {
    actions: { schema, text, same },
    start: () => startWalkthrough(browser, base),
    close: async () => { await browser.close(); server.kill(); },
  };
}

async function waitForServer(url) {
  for (const deadline = Date.now() + 10_000; Date.now() < deadline;) {
    try { if ((await fetch(url)).ok) return; } catch { /* not listening yet */ }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`${url} did not answer`);
}

/** A new browser context, so the library of each walkthrough starts empty. */
async function startWalkthrough(browser, base) {
  const context = await browser.newContext({ viewport: VIEWPORT, deviceScaleFactor: 1 });
  const pending = new Set();
  context.on('request', request => pending.add(request));
  for (const settled of ['requestfinished', 'requestfailed']) context.on(settled, request => pending.delete(request));
  const page = await context.newPage();
  await page.goto(`${base}/planner.html#/planner`);
  await page.locator('.maplibregl-canvas').waitFor();
  return {
    app: page,
    screenshot: () => stableScreenshot(page, pending),
    perform: action => perform(page, action),
    describe: action => describe(page, action),
    end: () => context.close(),
  };
}

/**
 * Two equal screenshots 300 ms apart with no request in flight; after 5 s the last one, marked unstable.
 * A page that waits for an answer can show the same pixels for longer than 300 ms, and the participant
 * has no action to wait.
 */
async function stableScreenshot(page, pending) {
  const shoot = () => page.screenshot({ animations: 'disabled', caret: 'hide' });
  let last = await shoot();
  for (const deadline = Date.now() + STABLE_LIMIT_MS; Date.now() < deadline;) {
    await page.waitForTimeout(STABLE_GAP_MS);
    const next = await shoot();
    if (next.equals(last) && !pending.size) return { png: next, unstable: false };
    last = next;
  }
  return { png: last, unstable: true };
}

async function perform(page, { type, x, y, x2, y2, text }) {
  const { mouse, keyboard } = page;
  if (type === 'click') await mouse.click(x, y);
  else if (type === 'hover') await mouse.move(x, y);
  else if (type === 'scroll') { await mouse.move(x, y); await mouse.wheel(0, text === 'up' ? -SCROLL_PX : SCROLL_PX); }
  else if (type === 'drag') { await mouse.move(x, y); await mouse.down(); await mouse.move(x2, y2, { steps: 10 }); await mouse.up(); }
  else if (type === 'type') { if (x !== null) await mouse.click(x, y); await keyboard.type(text ?? ''); }
  else if (type === 'key') await keyboard.press(text);
}

/** `screen`: the page from the address hash and the open dialogs and menus. `hit`: the control under the point. */
function describe(page, action) {
  return page.evaluate(({ x, y }) => {
    const label = el => (el.getAttribute('aria-label')
      || el.getAttribute('aria-labelledby')?.split(' ').map(id => document.getElementById(id)?.textContent).join(' ')
      || el.textContent || el.getAttribute('placeholder') || el.getAttribute('title') || '').replace(/\s+/g, ' ').trim().slice(0, 60);
    const role = el => el.getAttribute('role') ?? ({ A: 'link', BUTTON: 'button', SELECT: 'combobox', TEXTAREA: 'textbox', DIALOG: 'dialog' }[el.tagName]
      ?? (el.tagName === 'INPUT' ? (['checkbox', 'radio', 'range'].includes(el.type) ? el.type : 'textbox') : el.tagName.toLowerCase()));
    const visible = el => el.checkVisibility();
    const open = [...document.querySelectorAll('[role=dialog], [role=alertdialog], [role=menu], [role=listbox], dialog[open]')]
      .filter(visible).map(el => `${role(el)} "${label(el)}"`);
    const page = location.hash.replace(/^#\/?/, '').split('?')[0] || 'home';
    const screen = [page, ...open].join(' · ');
    const under = x === null ? document.activeElement : document.elementFromPoint(x, y);
    if (!under || under === document.body) return { screen, hit: null };
    if (under.closest('.maplibregl-canvas-container')) return { screen, hit: 'map' };
    const control = under.closest('button, a[href], input, select, textarea, [role]') ?? under;
    return { screen, hit: `${role(control)} "${label(control)}"` };
  }, action);
}
