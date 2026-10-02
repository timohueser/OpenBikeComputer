import { distance, around, routePosition } from './engine.mjs';

export const centre = (b) => [(b[0] + b[2]) / 2, (b[1] + b[3]) / 2];
export function lengths(line) {
  const out = [0];
  for (let i = 1; i < line.length; i++)
    out.push(out[i - 1] + distance(line[i - 1], line[i]));
  return out;
}
// A client sends a simplified line with the kilometres of its full line.
export const planKm = (context) =>
  context.plan?.km ?? lengths(context.plan?.coordinates ?? []);
export function at(line, km, ds = lengths(line)) {
  const total = ds.at(-1);
  if (!line.length) throw new Error('Add a route first.');
  if (km <= 0) return line[0];
  if (km >= total) return line.at(-1);
  const i = ds.findIndex((d) => d >= km),
    f = (km - ds[i - 1]) / (ds[i] - ds[i - 1] || 1);
  return line[i - 1].map((v, j) => v + f * (line[i][j] - v));
}
export function slice(line, from, to, ds = lengths(line)) {
  return [
    at(line, from, ds),
    ...line.filter((_, i) => ds[i] > from && ds[i] < to),
    at(line, to, ds),
  ];
}
export function dayNumber(value, context) {
  if (Number.isInteger(value)) return value;
  if (value === 'every')
    throw new Error('Choose a single day for this reference.');
  if (!context.startDate)
    throw new Error('Set the trip start date to use today or tomorrow.');
  const today = new Intl.DateTimeFormat('en-CA', {
    timeZone: 'Europe/Berlin',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(new Date(context.now || Date.now()));
  return (
    Math.round((Date.parse(today) - Date.parse(context.startDate)) / 86400000) +
    1 +
    (value === 'tomorrow' ? 1 : 0)
  );
}
export function dayRange(value, context) {
  const n = dayNumber(value, context),
    day = context.plan?.days?.find((d) => d.number === n);
  if (!day) throw new Error(`The plan has no Day ${n}.`);
  if (day.rest) throw new Error(`Day ${n} is a rest day.`);
  return [day.from, day.to];
}
export function kmQuantity(q, context) {
  if (q.unit === 'km') return q.value;
  if (q.unit === 'h') {
    if (!context.plan?.hours?.length)
      throw new Error(
        'Riding-time data is not available for this route. Use kilometres.',
      );
    const hours = context.plan.hours,
      ds = planKm(context);
    if (q.value < 0 || q.value > hours.at(-1))
      throw new Error('That riding-time mark is beyond the route.');
    const i = Math.max(
      1,
      hours.findIndex((h) => h >= q.value),
    );
    return (
      ds[i - 1] +
      ((ds[i] - ds[i - 1]) * (q.value - hours[i - 1])) /
        (hours[i] - hours[i - 1] || 1)
    );
  }
  throw new Error('Use kilometres or riding hours.');
}
export function alongRange(along, range, context) {
  const line = context.plan.coordinates,
    ds = planKm(context),
    total = ds.at(-1);
  let origin =
    along.ref === 'km' ? 0 : along.ref === 'end' ? range[1] : range[0];
  if (along.ref === 'here') {
    if (!context.here) throw new Error('Set your location to use “from here”.');
    const pos = routePosition(context.here, line, ds);
    if (pos.distance > 1)
      throw new Error('Your location is more than 1 km from the route.');
    origin = pos.along;
  }
  const sign = along.ref === 'end' ? -1 : 1;
  const mark = (q) => {
    if (q.unit === 'km') return origin + sign * q.value;
    const hours = context.plan.hours;
    if (!hours?.length)
      throw new Error(
        'Riding-time data is not available for this route. Use kilometres.',
      );
    const i = Math.max(
      1,
      ds.findIndex((d) => d >= origin),
    );
    const h =
      hours[i - 1] +
      ((hours[i] - hours[i - 1]) * (origin - ds[i - 1])) /
        (ds[i] - ds[i - 1] || 1);
    return kmQuantity({ value: h + sign * q.value, unit: 'h' }, context);
  };
  if (along.at) {
    const p = mark(along.at);
    if (p < 0 || p > total) throw new Error('That mark is beyond the route.');
    return [p, p];
  }
  const a = along.from ? mark(along.from) : origin,
    b = along.to ? mark(along.to) : sign > 0 ? range[1] : range[0];
  const out = [
    Math.max(range[0], Math.min(a, b)),
    Math.min(range[1], Math.max(a, b)),
  ];
  if (out[0] > out[1])
    throw new Error('That interval is outside this route section.');
  return out;
}
export function boxes(line, radius) {
  const out = [];
  for (let i = 0; i < line.length - 1; i++) {
    const b = line[i + 1];
    const a = line[i],
      pad = around(
        centre([
          Math.min(a[0], b[0]),
          Math.min(a[1], b[1]),
          Math.max(a[0], b[0]),
          Math.max(a[1], b[1]),
        ]),
        radius,
      );
    const dx = (pad[2] - pad[0]) / 2,
      dy = (pad[3] - pad[1]) / 2;
    const box = [
        Math.min(a[0], b[0]) - dx,
        Math.min(a[1], b[1]) - dy,
        Math.max(a[0], b[0]) + dx,
        Math.max(a[1], b[1]) + dy,
      ],
      last = out.at(-1);
    const merged = last
      ? [
          Math.min(last[0], box[0]),
          Math.min(last[1], box[1]),
          Math.max(last[2], box[2]),
          Math.max(last[3], box[3]),
        ]
      : null;
    if (
      merged &&
      merged[2] - merged[0] <= dx * 6 &&
      merged[3] - merged[1] <= dy * 6
    )
      out[out.length - 1] = merged;
    else out.push(box);
  }
  return out;
}

export function hoursAt(km, context) {
  const hours = context.plan?.hours,
    ds = planKm(context);
  if (!hours?.length)
    throw new Error(
      'Riding-time data is not available for this route. Use kilometres.',
    );
  const i = Math.max(
    1,
    ds.findIndex((d) => d >= km),
  );
  return (
    hours[i - 1] +
    ((hours[i] - hours[i - 1]) * (km - ds[i - 1])) / (ds[i] - ds[i - 1] || 1)
  );
}
export function crossesView(line, b) {
  return line.slice(1).some((end, i) => {
    const start = line[i];
    let lo = 0,
      hi = 1;
    for (let axis = 0; axis < 2; axis++) {
      const d = end[axis] - start[axis];
      if (!d) {
        if (start[axis] < b[axis] || start[axis] > b[axis + 2]) return false;
        continue;
      }
      const a = (b[axis] - start[axis]) / d,
        c = (b[axis + 2] - start[axis]) / d;
      lo = Math.max(lo, Math.min(a, c));
      hi = Math.min(hi, Math.max(a, c));
      if (lo > hi) return false;
    }
    return true;
  });
}
