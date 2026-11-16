// The model of the participant with its prices, the spending limit, and the tasks.

export const model = { id: 'claude-sonnet-5-5', usdPerMillionInput: 2, usdPerMillionOutput: 10 };
export const maxCostUsd = 15;
export const walkthroughsPerTask = 3;

/** True when a route in the library starts within `radiusM` of `from` and ends within `radiusM` of `to`. */
async function libraryHasRoute(page, from, to, radiusM = 1000) {
  const lines = await page.evaluate(() => new Promise((resolve, reject) => {
    const request = indexedDB.open('obc-library');
    request.onerror = () => reject(request.error);
    request.onsuccess = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains('routes')) return resolve([]);
      const all = db.transaction('routes').objectStore('routes').getAll();
      all.onsuccess = () => resolve(all.result.map(route => route.summary?.line ?? []));
      all.onerror = () => reject(all.error);
    };
  }));
  return lines.some(line => line.length >= 4
    && metres(line.slice(0, 2), from) <= radiusM && metres(line.slice(-2), to) <= radiusM);
}

/** The great-circle distance of two [longitude, latitude] points. */
function metres([lon1, lat1], [lon2, lat2]) {
  const rad = Math.PI / 180;
  const a = Math.sin((lat2 - lat1) * rad / 2) ** 2
    + Math.cos(lat1 * rad) * Math.cos(lat2 * rad) * Math.sin((lon2 - lon1) * rad / 2) ** 2;
  return 2 * 6_371_000 * Math.asin(Math.sqrt(a));
}

// Coordinates from the place search of the active planner release: the station on Bahnhofstrasse
// (OSM node 5007055738) and the pass (OSM node 1456026791).
// The search also lists a second "train station" result about 600 m from the station, so the radius is 1 km.
const MEIRINGEN_STATION = [8.184271, 46.727359];
const GRIMSEL_PASS = [8.337697, 46.561519];

export const tasks = [
  {
    id: 'plan-route',
    goal: 'You want to ride from Meiringen station to the top of the Grimsel Pass tomorrow. Plan the route.',
    stepBudget: 25,
    success: page => libraryHasRoute(page, MEIRINGEN_STATION, GRIMSEL_PASS),
  },
];
