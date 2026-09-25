export const duration = 60;
const radians = Math.PI / 180;
export const clamp = (x, low, high) => Math.min(high, Math.max(low, x));
const lerp = (a, b, t) => a + (b - a) * t;
export const angleDelta = (a, b) => Math.atan2(Math.sin(b - a), Math.cos(b - a));

function distance(a, b) {
  const dlat = (b.lat - a.lat) * radians, dlon = (b.lon - a.lon) * radians;
  const h = Math.sin(dlat / 2) ** 2 + Math.cos(a.lat * radians) * Math.cos(b.lat * radians) * Math.sin(dlon / 2) ** 2;
  return 12742000 * Math.asin(Math.sqrt(clamp(h, 0, 1)));
}

export function measure(points) {
  const result = [];
  for (const point of points) {
    if (![point.lat, point.lon, point.height].every(Number.isFinite)) throw new Error('The track contains an invalid point.');
    const previous = result.at(-1);
    const step = previous ? distance(previous, point) : 0;
    if (previous && step < 0.1) continue;
    result.push({ ...point, distance: (previous?.distance ?? 0) + step });
  }
  if (result.length < 2) throw new Error('The track needs at least two different points.');
  return result;
}

export function sample(track, meters) {
  meters = clamp(meters, 0, track.at(-1).distance);
  let low = 0, high = track.length - 1;
  while (high - low > 1) {
    const mid = (low + high) >> 1;
    if (track[mid].distance <= meters) low = mid;
    else high = mid;
  }
  const a = track[low], b = track[high];
  const t = (meters - a.distance) / (b.distance - a.distance);
  return { lon: lerp(a.lon, b.lon, t), lat: lerp(a.lat, b.lat, t),
    height: lerp(a.height, b.height, t), distance: meters };
}

function bearing(a, b) {
  const delta = (b.lon - a.lon) * radians;
  return Math.atan2(Math.sin(delta) * Math.cos(b.lat * radians),
    Math.cos(a.lat * radians) * Math.sin(b.lat * radians) -
    Math.sin(a.lat * radians) * Math.cos(b.lat * radians) * Math.cos(delta));
}

// Precomputed poses make seeking independent of the order in which frames are visited.
export function cameraPlan(track) {
  const total = track.at(-1).distance, frames = duration * 30;
  let heading = bearing(sample(track, 0), sample(track, 1200));
  const poses = [];
  for (let i = 0; i <= frames; i++) {
    const meters = total * i / frames;
    const behind = sample(track, meters - 350), ahead = sample(track, meters + 1000);
    const wanted = bearing(behind, ahead);
    heading += clamp(angleDelta(heading, wanted), -25 * radians / 30, 25 * radians / 30);
    const relief = Math.abs(ahead.height - behind.height);
    poses.push({ heading, pitch: (-32 - Math.min(relief / 45, 9)) * radians,
      range: 2100 + Math.min(relief * 1.5, 550), target: Math.min(total, meters + 180) });
  }
  return poses;
}

export function cameraAt(plan, progress) {
  const position = clamp(progress, 0, 1) * (plan.length - 1);
  const i = Math.min(Math.floor(position), plan.length - 2), t = position - i;
  const a = plan[i], b = plan[i + 1];
  return Object.fromEntries(['heading', 'pitch', 'range', 'target'].map(key => [key, lerp(a[key], b[key], t)]));
}
