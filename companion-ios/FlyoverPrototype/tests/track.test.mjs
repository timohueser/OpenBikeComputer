import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { measure, sample, cameraPlan, cameraAt, duration, angleDelta } from '../Web/track.mjs';

const gpx = readFileSync(new URL('../Web/kandel.gpx', import.meta.url), 'utf8');
const points = [...gpx.matchAll(/<trkpt lat="([^"]+)" lon="([^"]+)">\s*<ele>([^<]+)<\/ele>/g)]
  .map(([, lat, lon, height]) => ({ lat: +lat, lon: +lon, height: +height }));
const track = measure(points);

test('the complete Kandel ride has a finite pose at every frame and no abrupt turn', () => {
  assert.ok(track.at(-1).distance > 58000 && track.at(-1).distance < 60000);
  const plan = cameraPlan(track);
  for (let frame = 0; frame <= duration * 60; frame++) {
    const pose = cameraAt(plan, frame / (duration * 60));
    assert.ok(Object.values(pose).every(Number.isFinite));
    if (frame) {
      const previous = cameraAt(plan, (frame - 1) / (duration * 60));
      assert.ok(Math.abs(angleDelta(previous.heading, pose.heading)) <= 25 * Math.PI / 180 / 60 + 1e-10);
    }
  }
});

test('backwards seeking gives exactly the same pose, location and height', () => {
  const plan = cameraPlan(track), end = track.at(-1).distance;
  const forward = [0, 0.25, 0.5, 0.75, 1].map(t => [cameraAt(plan, t), sample(track, t * end)]);
  const backwards = [1, 0.75, 0.5, 0.25, 0].map(t => [cameraAt(plan, t), sample(track, t * end)]).reverse();
  assert.deepEqual(backwards, forward);
  assert.equal(sample(track, -100).distance, 0);
  assert.equal(sample(track, end + 100).distance, end);
});

test('a tight out-and-back keeps its camera turn bounded at the reversal', () => {
  const hairpin = measure([
    { lat: 48, lon: 8, height: 500 }, { lat: 48.03, lon: 8, height: 700 },
    { lat: 48.03, lon: 8.001, height: 705 }, { lat: 48, lon: 8.001, height: 900 },
  ]);
  const plan = cameraPlan(hairpin);
  for (let i = 1; i < plan.length; i++) {
    assert.ok(Math.abs(angleDelta(plan[i - 1].heading, plan[i].heading)) <= 25 * Math.PI / 180 / 30 + 1e-10);
  }
  assert.throws(() => measure([hairpin[0], hairpin[0]]), /two different points/);
});
