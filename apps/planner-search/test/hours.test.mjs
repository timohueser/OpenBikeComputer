import { test } from 'node:test';
import assert from 'node:assert/strict';
import { currentOpening } from '../hours.mjs';
const place = { lat:48.13, lon:7.81, region:'Baden-Württemberg', opening_hours:'Mo-Fr 08:30-13:00,15:00-18:30; PH off' };
const status = (date, tag = place.opening_hours) => currentOpening({...place,opening_hours:tag}, Date.parse(date));

test('current status respects local time, split shifts, and the next closure', () => {
  const morning=status('2026-09-28T10:33:00Z');
  assert.equal(morning.state,'open');
  assert.equal(morning.closesAt,Date.parse('2026-09-28T11:00:00Z'));
  assert.equal(status('2026-09-28T11:00:00Z').state,'closed');
  assert.equal(status('2026-09-28T13:00:00Z').state,'open');
  assert.equal(status('2026-09-28T16:30:00Z').state,'closed');
  assert.equal(status('2026-12-28T11:33:00Z').closesAt,Date.parse('2026-12-28T12:00:00Z'));
});
test('holidays and overnight hours are evaluated without guessing unknown schedules', () => {
  assert.equal(status('2026-12-25T09:00:00Z').state,'closed');
  assert.equal(status('2026-09-28T23:33:00Z','Mo 22:00-02:00').closesAt,Date.parse('2026-09-29T00:00:00Z'));
  assert.equal(status('2026-09-28T10:00:00Z','"by appointment"').state,'unknown');
  assert.equal(status('2026-09-28T10:00:00Z','not hours').state,'unknown');
  assert.equal(currentOpening({}),undefined);
});
test('snapshots expire by their next transition and do not invent a 24/7 closure', () => {
  const closing=status('2026-09-28T10:58:00Z');
  assert.equal(closing.validUntil,closing.closesAt);
  const always=status('2026-09-28T10:00:00Z','24/7');
  assert.equal(always.state,'open');
  assert.equal(always.closesAt,undefined);
  assert.equal(always.validUntil-always.checkedAt,300_000);
});
