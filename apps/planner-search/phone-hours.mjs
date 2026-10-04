import {openingHours} from './hours.mjs';

const schedules = [
  'Mo-Fr 08:30-13:00,15:00-18:30; PH off', 'Mo 22:00-02:00', '24/7',
  'Mo,We-Fr 13:00+; Sa,Su 10:30+; Tu off', '"by appointment"', 'not hours',
  'sunrise-sunset', 'PH 10:00-16:00; Mo-Fr 08:00-18:00', 'SH off; Mo-Su 09:00-18:00',
  'Jan-Mar off; Apr-Oct 09:00-18:00', 'Sa[1] 09:00-12:00', 'Su 01:00-04:00',
];
const dates = [
  '2026-09-28T10:33:00Z','2026-09-28T11:00:00Z','2026-09-28T13:00:00Z','2026-09-28T23:33:00Z',
  '2026-12-25T09:00:00Z','2026-12-28T11:33:00Z','2026-03-29T00:59:00Z','2026-03-29T01:00:00Z',
  '2026-10-25T00:59:00Z','2026-10-25T01:00:00Z','2026-06-21T03:00:00Z','2026-07-30T10:00:00Z',
];

export function run(_all,digest,hours=openingHours('Europe/Berlin')) {
  const samples = [];
  for (const schedule of schedules) {
    const place = {lat:48.13,lon:7.81,region:'Baden-Württemberg',country:'de',opening_hours:schedule};
    for (const now of dates) {
      const started = performance.now();
      const result = {current:hours.currentOpening(place,Date.parse(now)),filtered:hours.openingState(place,{now:true},{now})};
      samples.push({kind:'hours',input:{schedule,now},elapsedMs:performance.now()-started,result,sha256:digest(JSON.stringify(result))});
    }
    for (const weekday of ['mon','tue','wed','thu','fri','sat','sun']) {
      const started = performance.now(), result = hours.openingState(place,{weekday},{});
      samples.push({kind:'hours',input:{schedule,weekday},elapsedMs:performance.now()-started,result,sha256:digest(JSON.stringify(result))});
    }
    for (const openDate of ['2026-12-25','2026-07-30']) {
      const started = performance.now(), result = hours.openingState(place,{}, {openDate});
      samples.push({kind:'hours',input:{schedule,openDate},elapsedMs:performance.now()-started,result,sha256:digest(JSON.stringify(result))});
    }
  }
  return {scope:'Shared opening_hours evaluator',calendarZone:hours.timeZone,
    hostTimeZone:new Intl.DateTimeFormat('en').resolvedOptions().timeZone,samples};
}
