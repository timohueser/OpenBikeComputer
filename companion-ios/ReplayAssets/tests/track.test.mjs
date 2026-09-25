import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeTrack, segments, sample, cameraPlan, cameraAt, adjustedPose, terrainSamples, Playback, angleDelta } from '../../Packages/OBCKit/Sources/OBCUI/Resources/Replay/track.mjs';

const point = (latitude, longitude, distance, elevation = 100, segmentStart = false) => ({ latitude, longitude, distance, elevation, segmentStart });
const close = (a, b, tolerance = 1e-8) => assert.ok(Math.abs(a - b) < tolerance, `${a} differs from ${b}`);
const mountain = normalizeTrack([
  point(48, 8, 0), point(48.01, 8, 1100, 300), point(48.01, 8.003, 1350, 350),
  point(48, 8.003, 2450, 500), point(48, 8.006, 2700, 600), point(48.01, 8.006, 3800, 700),
]);

test('hairpin plan bounds angular motion and repeated seeks are deterministic', () => {
  const plan = cameraPlan(mountain, 60);
  for (let index = 1; index < plan[0].poses.length; index++) {
    assert.ok(Math.abs(plan[0].poses[index].heading - plan[0].poses[index - 1].heading) <= 25 * Math.PI / 180 / 30 + 1e-10);
  }
  const expected = [0, 1200, 2500, 3799, 3800].map(distance => cameraAt(plan, distance));
  for (const distance of [3800, 3, 2700, 0, 1000, 2600]) cameraAt(plan, distance);
  assert.deepEqual([0, 1200, 2500, 3799, 3800].map(distance => cameraAt(plan, distance)), expected);
});

test('heading follows the short path through north and unwraps dateline coordinates', () => {
  close(angleDelta(359 * Math.PI / 180, Math.PI / 180), 2 * Math.PI / 180);
  const track = normalizeTrack([point(0, 179.9, 0), point(0, -179.9, 20000)]);
  assert.equal(Math.abs(sample(track, 10000).lon), 180);
  const pose = cameraAt(cameraPlan(track, 60), 10000);
  close(pose.heading, Math.PI / 2);
});

test('stationary, repeated and missing-height samples have finite camera poses', () => {
  const track = normalizeTrack([point(48, 8, 0, null), point(48, 8, 0, null)]);
  assert.equal(sample(track, 0).height, null);
  assert.equal(normalizeTrack([{ latitude: 48, longitude: 8, distance: 0 }])[0].height, null);
  const pose = cameraAt(cameraPlan(track, 60), 0);
  assert.ok(Object.values(pose).every(Number.isFinite));
  assert.equal(terrainSamples(track).length, 1);
  const state = new Playback(0, 60);
  state.play(); state.advance(100);
  assert.equal(state.playing, false);
  assert.equal(state.distance, 0);
});

test('segment boundary jumps to the next start without interpolation across the gap', () => {
  const track = normalizeTrack([point(48, 8, 0), point(48.01, 8, 1000),
    point(50, 10, 1000, 200, true), point(50.01, 10, 2000)]);
  assert.ok(sample(track, 999).lat < 48.02);
  assert.equal(sample(track, 1000).lat, 50);
  assert.equal(sample(track, 1000).segment, 1);
  const ground = terrainSamples(track);
  assert.equal(segments(ground).length, 2);
  assert.ok(segments(ground)[0].every(p => p.lat < 49));
  assert.ok(segments(ground)[1].every(p => p.lat >= 50));
  const plan = cameraPlan(track, 60);
  assert.equal(cameraAt(plan, 999).segment, 0);
  assert.equal(cameraAt(plan, 1000).segment, 1);
  assert.ok(cameraAt(plan, 999).target <= 1000);
});

test('terrain budget retains short segments and limits work for a long ride', () => {
  const track = normalizeTrack([point(1, 1, 0), point(2, 2, 500000),
    point(10, 10, 500000, null, true), point(10.001, 10.001, 500001),
    point(20, 20, 500001, null, true)]);
  const ground = terrainSamples(track, 100);
  assert.ok(ground.length <= 100);
  assert.equal(segments(ground).length, 3);
  assert.equal(segments(ground)[1].length, 2);
  assert.equal(ground.at(-1).lat, 20);
  assert.throws(() => terrainSamples(track, 3), /too many/);
});

test('adjusted follow stays rider-relative through turns, pause, seek and overview', () => {
  const state = new Playback(3800, 60), plan = cameraPlan(mountain, 60);
  state.play(); state.advance(15); state.adjust(0.4, -0.1, 1.2);
  const before = { ...state.offsets }, distance = state.distance;
  state.pause(); state.camera('overview'); state.camera('overview');
  state.seek(2700); state.camera('follow');
  assert.equal(state.mode, 'adjusted');
  assert.deepEqual(state.offsets, before);
  for (const meters of [distance, 2700]) {
    const automatic = cameraAt(plan, meters), adjusted = adjustedPose(automatic, state.offsets);
    close(adjusted.heading - automatic.heading, 0.4);
    close(adjusted.range / automatic.range, 1.2);
  }
  state.camera('auto');
  assert.equal(state.distance, 2700);
  assert.equal(state.playing, false);
  assert.equal(state.mode, 'auto');
  assert.deepEqual(state.offsets, { heading: 0, pitch: 0, range: 1 });
});

test('overview restores auto and an overview gesture enters adjusted follow', () => {
  const state = new Playback(1000, 60);
  state.camera('overview'); state.play(); state.advance(3);
  assert.equal(state.mode, 'overview');
  assert.equal(state.distance, 50);
  state.camera('follow'); assert.equal(state.mode, 'auto');
  state.camera('overview'); state.adjust(0.2, 0, 1);
  assert.equal(state.mode, 'adjusted');
  assert.equal(state.playing, true);
});

test('playback stops exactly at the end, replay restarts, and seek clamps and pauses', () => {
  const state = new Playback(1000, 60);
  state.speed = 2; state.play(); state.advance(40);
  assert.equal(state.distance, 1000); assert.equal(state.playing, false);
  state.play(); assert.equal(state.distance, 0);
  state.seek(2000); assert.equal(state.distance, 1000); assert.equal(state.playing, false);
  state.seek(-10); assert.equal(state.distance, 0);
  state.seek(NaN); assert.equal(state.distance, 0);
});

test('invalid geometry and duration fail at the boundary', () => {
  assert.throws(() => normalizeTrack([]), /no recorded/);
  for (const input of [[point(91, 0, 0)], [point(1, 1, 1)], [point(1, 1, 0, NaN)],
    [point(1, 1, 0), point(2, 2, -1)]]) assert.throws(() => normalizeTrack(input), /invalid/);
  assert.throws(() => new Playback(1, Infinity), /duration/);
  assert.throws(() => new Playback(1, 0), /duration/);
});

test('camera snapshot restores adjusted and remembered overview framing after renderer replacement', () => {
  const source = new Playback(3800, 60);
  source.adjust(0.8, -0.2, 1.5);
  for (const overview of [false, true]) {
    if (overview) source.camera('overview');
    const snapshot = source.cameraSnapshot();
    const replacement = new Playback(3800, 60);
    assert.equal(replacement.restoreCamera(snapshot), true);
    assert.deepEqual(replacement.cameraSnapshot(), snapshot);
    assert.equal(replacement.playing, false);
    replacement.camera('follow');
    assert.equal(replacement.mode, 'adjusted');
    assert.deepEqual(replacement.offsets, source.offsets);
  }
  const before = source.cameraSnapshot();
  for (const invalid of [null, {}, { ...before, followMode: 'overview' },
    { ...before, offsets: { heading: NaN, pitch: 0, range: 1 } },
    { ...before, offsets: { heading: 0, pitch: 0, range: 10 } }]) {
    assert.equal(source.restoreCamera(invalid), false);
    assert.deepEqual(source.cameraSnapshot(), before);
  }
});
