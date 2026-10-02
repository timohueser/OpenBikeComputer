import { test } from 'node:test';
import assert from 'node:assert/strict';
import { currentOpening, openingHours } from '../hours.mjs';
import vm from 'node:vm';
import { calendarDate } from '../calendar-date.mjs';
import { calendarBundle } from '../calendar-bundle.mjs';
import { run as hoursCases } from '../phone-hours.mjs';
process.env.TZ = 'Europe/Berlin';
const place = { lat:48.13, lon:7.81, region:'Baden-Württemberg', opening_hours:'Mo-Fr 08:30-13:00,15:00-18:30; PH off' };
const status = (date, tag = place.opening_hours) => currentOpening({...place,opening_hours:tag}, Date.parse(date));

test('current status respects local time, split shifts, and the next closure', () => {
  const morning=status('2026-09-28T10:33:00Z');
  assert.equal(morning.state,'open');
  assert.deepEqual(morning,{state:'open',closesAt:'13:00'});
  assert.equal(status('2026-09-28T11:00:00Z').state,'closed');
  assert.equal(status('2026-09-28T13:00:00Z').state,'open');
  assert.equal(status('2026-09-28T16:30:00Z').state,'closed');
  assert.equal(status('2026-12-28T11:33:00Z').closesAt,'13:00');
});
test('holidays and overnight hours are evaluated without guessing unknown schedules', () => {
  assert.equal(status('2026-12-25T09:00:00Z').state,'closed');
  assert.equal(status('2026-09-28T23:33:00Z','Mo 22:00-02:00').closesAt,'02:00');
  assert.equal(status('2026-09-28T10:00:00Z','"by appointment"').state,'unknown');
  assert.equal(status('2026-09-28T10:00:00Z','not hours').state,'unknown');
  assert.equal(currentOpening({}),undefined);
});
test('a 24/7 place has no closing time', () => {
  assert.deepEqual(status('2026-09-28T10:00:00Z','24/7'),{state:'open'});
});

test('open-ended hours do not imply a known closing time or an open badge', () => {
  const current = status('2026-09-28T12:00:00Z', 'Mo,We-Fr 13:00+; Sa,Su 10:30+; Tu off');
  assert.equal(current.state, 'unknown');
  assert.equal(current.closesAt, undefined);
});

test('opening-hours adapters require an explicit matching calendar zone', () => {
  assert.throws(()=>openingHours({countryCode:'de'}),/time zone/);
  const hours = openingHours({countryCode:'de',timeZone:'Europe/Berlin'});
  assert.doesNotThrow(()=>hours.assertEnvironment());
  const utc = openingHours({countryCode:'de',timeZone:'UTC'});
  assert.throws(()=>utc.assertEnvironment(),/calendar runtime/);
});

test('portable hours preserve all calendar results outside the region time zone', async () => {
  const expected = hoursCases(null, ()=>'').samples.map(row=>row.result);
  const bundle = await calendarBundle();
  try {
    for (const zone of ['UTC', 'America/New_York', 'Asia/Tokyo', 'Pacific/Auckland']) {
      process.env.TZ = zone;
      const context = vm.createContext({});
      vm.runInContext('globalThis.originalDate = Date', context);
      vm.runInContext(bundle, context);
      const hours = vm.runInContext("PlannerCalendar.openingHours({countryCode:'de',timeZone:'Europe/Berlin'})", context);
      const actual = hoursCases(null, () => '', hours).samples.map(row=>row.result);
      assert.deepEqual(JSON.parse(JSON.stringify(actual)), expected, zone);
      assert.equal(vm.runInContext('Date === originalDate', context), true);
      assert.equal(new Intl.DateTimeFormat('en').resolvedOptions().timeZone, zone);
    }
  } finally { process.env.TZ = 'Europe/Berlin'; }
});

test('regional constructors and setters match native DST gap and overlap disambiguation', () => {
  const inputs = [];
  for (let year=2024; year<=2030; year++) for (const month of [2,9])
    for (let day=20; day<=31; day++) for (let hour=0; hour<5; hour++)
      for (let minute=0; minute<60; minute+=15) inputs.push([year,month,day,hour,minute]);
  inputs.push([2026,0,0], [2026,12,32,-1], [NaN], [undefined],
    ['2026-03-29T02:30:00'], ['2026-10-25T02:30:00'], ['2026-10-25'], ['2026-10-25T01:00:00Z']);
  const evaluate = Calendar => inputs.map(args => {
    const date = new Calendar(...args), result = [+date,date.getDay(),date.getTimezoneOffset()];
    date.setHours(2,30);
    result.push(+date);
    date.setDate(date.getDate()+1);
    result.push(+date);
    return result;
  });
  const expected = evaluate(Date), Calendar = calendarDate('Europe/Berlin');
  try {
    for (const zone of ['Europe/Berlin','UTC','America/New_York','Asia/Tokyo','Pacific/Auckland']) {
      process.env.TZ = zone;
      assert.deepEqual(evaluate(Calendar), expected, zone);
    }
  } finally { process.env.TZ = 'Europe/Berlin'; }
});
