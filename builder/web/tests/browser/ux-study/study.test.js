import assert from 'node:assert/strict';
import { test } from 'node:test';
import { tasks } from './study.js';

const MEIRINGEN = [8.184271, 46.727359];
const GRIMSEL = [8.337697, 46.561519];
const FURKA = [8.415184, 46.572685];
const ANDERMATT = [8.594815, 46.63405];
const LUCERNE = [8.305812, 47.052144];
const ALTDORF = [8.6440, 46.8805];

/** The point at `share` of the straight way from `a` to `b`. */
const at = (a, b, share) => [a[0] + (b[0] - a[0]) * share, a[1] + (b[1] - a[1]) * share];
/** A summary line through `points`, with ten points on each straight part. */
const line = (...points) => points.slice(1).flatMap((b, i) =>
  Array.from({ length: 10 }, (_, k) => at(points[i], b, k / 10))).concat([points.at(-1)]).flat();
const route = (points, { days = 1, bike = 'touring', stops = [] } = {}) =>
  ({ summary: { line: line(...points), days, bike }, trip: { points: stops } });
const stop = (kind, placeKind, share) => ({ kind, placeKind, coordinate: at(MEIRINGEN, GRIMSEL, share) });

const cases = [
  ['plan-route', route([MEIRINGEN, GRIMSEL]), true],
  ['plan-route', route([GRIMSEL, MEIRINGEN]), false],
  ['route-through', route([MEIRINGEN, GRIMSEL, FURKA, ANDERMATT]), true],
  ['route-through', route([GRIMSEL, FURKA, ANDERMATT]), false],
  ['route-through', route([MEIRINGEN, GRIMSEL, ANDERMATT]), false],
  ['route-through', route([MEIRINGEN, GRIMSEL, FURKA]), false],
  ['two-day-trip', route([LUCERNE, ANDERMATT], { days: 2 }), true],
  ['two-day-trip', route([LUCERNE, ANDERMATT]), false],
  ['two-day-trip', route([MEIRINGEN, ANDERMATT], { days: 2 }), false],
  ['two-day-trip', route([LUCERNE, ALTDORF], { days: 2 }), false],
  ['change-bike', route([MEIRINGEN, GRIMSEL], { bike: 'mtb' }), true],
  ['change-bike', route([MEIRINGEN, GRIMSEL], { bike: 'gravel' }), false],
  ['lunch-stop', route([MEIRINGEN, GRIMSEL], { stops: [stop('waypoint', 'restaurant', 0.5)] }), true],
  ['lunch-stop', route([MEIRINGEN, GRIMSEL], { stops: [stop('detour', 'cafe', 0.6)] }), true],
  ['lunch-stop', route([MEIRINGEN, GRIMSEL], { stops: [stop('pass', 'restaurant', 0.4)] }), true],
  ['lunch-stop', route([MEIRINGEN, GRIMSEL], { stops: [stop('waypoint', 'restaurant', 0.1)] }), false],
  ['lunch-stop', route([MEIRINGEN, GRIMSEL], { stops: [stop('marker', 'restaurant', 0.5)] }), false],
  ['lunch-stop', route([MEIRINGEN, GRIMSEL], { stops: [stop('waypoint', 'hotel', 0.5)] }), false],
  ['lunch-stop', route([MEIRINGEN, ANDERMATT], { stops: [stop('waypoint', 'restaurant', 0.5)] }), false],
];

test('each task accepts exactly the route records that reach its goal', () => {
  const byId = Object.fromEntries(tasks.map(task => [task.id, task]));
  const wrong = cases.map(([id, record, fits], i) => byId[id].fits(record) === fits ? null : `case ${i + 1} (${id})`)
    .filter(Boolean);
  assert.deepEqual(wrong, []);
});
