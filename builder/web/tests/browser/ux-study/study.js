// The models with their prices, the spending limit, and the tasks.
import placeKinds from '../../../../../planner/search/place-kinds.json' with { type: 'json' };

// USD for 1 million input and output tokens. Haiku costs five times as much for a call with more than
// 100K input tokens; the budget uses the lower price.
export const models = [
  { id: 'claude-sonnet-5-5', usdPerMillionInput: 2, usdPerMillionOutput: 10 },
  { id: 'claude-opus-5-5', usdPerMillionInput: 4, usdPerMillionOutput: 20 },
  { id: 'claude-haiku-5-5', usdPerMillionInput: 0.1, usdPerMillionOutput: 0.5 },
];
export const defaultModel = 'claude-sonnet-5-5';
export const maxCostUsd = 15;
export const walkthroughsPerTask = 3;

// The place search can list a second result for one place, such as a second "train station" result about
// 600 m from Meiringen station.
const RADIUS_M = 1000;

/**
 * The route records of the library that have a line. A record also holds the plan (`trip`) in the format of
 * the planner.
 */
async function libraryRoutes(page) {
  // The open has no version, so on a page whose app has not created the library yet it would create an
  // empty one and block the app's own open. It aborts that upgrade, and closes its connection after the read.
  const routes = await page.evaluate(() => new Promise((resolve, reject) => {
    const request = indexedDB.open('obc-library');
    let created = false;
    request.onupgradeneeded = () => { created = true; request.transaction.abort(); };
    request.onerror = () => (created ? resolve([]) : reject(request.error));
    request.onsuccess = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains('routes')) { db.close(); return resolve([]); }
      const all = db.transaction('routes').objectStore('routes').getAll();
      all.onsuccess = () => { db.close(); resolve(all.result.map(({ summary, trip }) => ({ summary, trip }))); };
      all.onerror = () => { db.close(); reject(all.error); };
    };
  }));
  return routes.filter(route => route.summary?.line.length >= 4);
}

/** The [longitude, latitude] points of a summary line, which stores them one after the other. */
const points = line => Array.from({ length: line.length / 2 }, (_, i) => [line[2 * i], line[2 * i + 1]]);
const near = (a, b) => metres(a, b) <= RADIUS_M;
const startsAt = (line, place) => near(line.slice(0, 2), place);
const endsAt = (line, place) => near(line.slice(-2), place);
const passes = (line, place) => points(line).some(point => near(point, place));

/** The great-circle distance of two [longitude, latitude] points. */
function metres([lon1, lat1], [lon2, lat2]) {
  const rad = Math.PI / 180;
  const a = Math.sin((lat2 - lat1) * rad / 2) ** 2
    + Math.cos(lat1 * rad) * Math.cos(lat2 * rad) * Math.sin((lon2 - lon1) * rad / 2) ** 2;
  return 2 * 6_371_000 * Math.asin(Math.sqrt(a));
}

/** The share of the length of `line` that lies before its point nearest to `place`: 0 at the start, 1 at the end. */
function shareAlong(line, place) {
  const all = points(line);
  const before = [0];
  for (let i = 1; i < all.length; i++) before.push(before[i - 1] + metres(all[i - 1], all[i]));
  const nearest = all.reduce((best, point, i) => (metres(point, place) < metres(all[best], place) ? i : best), 0);
  return before[nearest] / before.at(-1);
}

// Coordinates from the place search of the active planner release, with their OSM nodes: Meiringen station on
// Bahnhofstrasse (5007055738), the Grimsel Pass (1456026791), the Furka Pass (1457531552) and the village of
// Andermatt (240030058).
const MEIRINGEN_STATION = [8.184271, 46.727359];
const GRIMSEL_PASS = [8.337697, 46.561519];
const FURKA_PASS = [8.415184, 46.572685];
const ANDERMATT = [8.594815, 46.63405];
const FOOD = new Set(placeKinds.food);

const meiringenToGrimsel = ({ summary }) => startsAt(summary.line, MEIRINGEN_STATION) && endsAt(summary.line, GRIMSEL_PASS);
/** A stop of the plan at a place to eat, in the middle half of the line. */
const lunchStop = ({ summary, trip }) => (trip?.points ?? []).some(point => point.kind === 'waypoint'
  && FOOD.has(point.placeKind) && Math.abs(shareAlong(summary.line, point.coordinate) - 0.5) <= 0.25);
const libraryHas = (page, fits) => libraryRoutes(page).then(routes => routes.some(fits));

export const tasks = [
  {
    id: 'plan-route',
    goal: 'You want to ride from Meiringen station to the top of the Grimsel Pass tomorrow. Plan the route.',
    stepBudget: 25,
    success: page => libraryHas(page, meiringenToGrimsel),
  },
  {
    id: 'route-through',
    goal: 'You want to ride from Meiringen to Andermatt via the Grimsel Pass and the Furka Pass. Plan the route.',
    stepBudget: 25,
    success: page => libraryHas(page, ({ summary: { line } }) =>
      endsAt(line, ANDERMATT) && passes(line, GRIMSEL_PASS) && passes(line, FURKA_PASS)),
  },
  {
    id: 'two-day-trip',
    goal: 'You want to ride from Lucerne to Andermatt in two days. Plan the ride.',
    stepBudget: 25,
    success: page => libraryHas(page, ({ summary }) => endsAt(summary.line, ANDERMATT) && summary.days === 2),
  },
  {
    id: 'change-bike',
    goal: 'You want to ride from Meiringen station to the top of the Grimsel Pass on your mountain bike. Plan a route that suits your bike.',
    stepBudget: 25,
    success: page => libraryHas(page, route => meiringenToGrimsel(route) && route.summary.bike === 'mtb'),
  },
  {
    id: 'lunch-stop',
    goal: 'You want to ride from Meiringen station to the top of the Grimsel Pass, and stop for lunch about halfway. Plan the ride with a place to eat on the way.',
    stepBudget: 25,
    success: page => libraryHas(page, route => meiringenToGrimsel(route) && lunchStop(route)),
  },
];
