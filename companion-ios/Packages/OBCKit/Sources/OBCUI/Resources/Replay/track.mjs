export const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
const lerp = (a, b, t) => a + (b - a) * t;
const radians = Math.PI / 180;
export const angleDelta = (a, b) => Math.atan2(Math.sin(b - a), Math.cos(b - a));
const longitude = value => ((value + 180) % 360 + 360) % 360 - 180;

export function normalizeTrack(input) {
  if (!Array.isArray(input) || !input.length) throw new Error('This ride has no recorded points.');
  let previous = 0, segment = -1;
  return input.map((point, index) => {
    const { latitude: lat, longitude: lon, distance } = point;
    const height = point.elevation ?? null;
    if (![lat, lon, distance].every(Number.isFinite) || Math.abs(lat) > 90 || Math.abs(lon) > 180 ||
        distance < previous || (!index && distance !== 0) || (height !== null && !Number.isFinite(height))) {
      throw new Error('This ride contains an invalid recorded point.');
    }
    if (!index || point.segmentStart) segment++;
    previous = distance;
    return { lat, lon, height, distance, segment };
  });
}

export function segments(track) {
  const result = [];
  for (const point of track) {
    if (result.at(-1)?.[0].segment !== point.segment) result.push([]);
    result.at(-1).push(point);
  }
  return result;
}

export function sample(track, meters) {
  meters = clamp(meters, track[0].distance, track.at(-1).distance);
  let low = 0, high = track.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (track[middle].distance <= meters) low = middle + 1;
    else high = middle;
  }
  const a = track[Math.max(0, low - 1)], b = track[Math.min(low, track.length - 1)];
  if (a.segment !== b.segment || a.distance === b.distance) return { ...a, distance: meters };
  const t = (meters - a.distance) / (b.distance - a.distance);
  return { lat: lerp(a.lat, b.lat, t), lon: longitude(a.lon + angleDelta(a.lon * radians, b.lon * radians) / radians * t),
    height: a.height === null || b.height === null ? null : lerp(a.height, b.height, t),
    distance: meters, segment: a.segment };
}

function bearing(a, b, fallback) {
  if (Math.abs(a.lat - b.lat) + Math.abs(angleDelta(a.lon * radians, b.lon * radians)) < 1e-10) return fallback;
  const delta = angleDelta(a.lon * radians, b.lon * radians);
  return Math.atan2(Math.sin(delta) * Math.cos(b.lat * radians),
    Math.cos(a.lat * radians) * Math.sin(b.lat * radians) -
    Math.sin(a.lat * radians) * Math.cos(b.lat * radians) * Math.cos(delta));
}

// Each segment has its own plan. No camera interpolation spans a recording gap.
export function cameraPlan(track, durationSeconds) {
  const total = track.at(-1).distance;
  return segments(track).map(points => {
    const start = points[0].distance, end = points.at(-1).distance;
    const seconds = total ? durationSeconds * (end - start) / total : 1;
    const count = Math.max(1, Math.ceil(seconds * 30));
    const dt = seconds / count;
    let heading = bearing(points[0], sample(points, start + 1000), 0);
    const poses = [];
    for (let i = 0; i <= count; i++) {
      const distance = lerp(start, end, i / count);
      const behind = sample(points, distance - 350), ahead = sample(points, distance + 1000);
      heading += clamp(angleDelta(heading, bearing(behind, ahead, heading)), -25 * radians * dt, 25 * radians * dt);
      const relief = behind.height === null || ahead.height === null ? 0 : Math.abs(ahead.height - behind.height);
      poses.push({ distance, heading, pitch: (-32 - Math.min(relief / 45, 9)) * radians,
        range: 2100 + Math.min(relief * 1.5, 550), target: Math.min(end, distance + 180) });
    }
    return { start, end, segment: points[0].segment, poses };
  });
}

export function cameraAt(plan, distance) {
  let section = plan[0];
  for (const candidate of plan) {
    if (candidate.start > distance) break;
    section = candidate;
  }
  const { start, end, poses } = section;
  const index = end === start ? 0 : clamp((distance - start) / (end - start), 0, 1) * (poses.length - 1);
  const a = poses[Math.floor(index)], b = poses[Math.min(Math.floor(index) + 1, poses.length - 1)];
  const pose = Object.fromEntries(['heading', 'pitch', 'range', 'target'].map(key => [key, lerp(a[key], b[key], index % 1)]));
  return { ...pose, segment: section.segment };
}

export function adjustedPose(pose, offsets) {
  return { ...pose, heading: pose.heading + offsets.heading,
    pitch: clamp(pose.pitch + offsets.pitch, -85 * radians, -20 * radians),
    range: clamp(pose.range * offsets.range, 350, 12000) };
}

// Bound provider work while retaining both ends of every recorded segment.
export function terrainSamples(track, budget = 1200) {
  const parts = segments(track), minimum = parts.reduce((sum, part) => sum + Math.min(2, part.length), 0);
  if (minimum > budget) throw new Error('This ride has too many separate recording segments to load.');
  const total = track.at(-1).distance, spare = budget - minimum;
  return parts.flatMap(part => {
    const start = part[0].distance, end = part.at(-1).distance;
    if (part.length === 1 || end === start) return [{ ...part.at(-1) }];
    const count = Math.min(Math.max(1, Math.ceil((end - start) / 60)), 1 + Math.floor(spare * (end - start) / Math.max(1, total)));
    return Array.from({ length: count + 1 }, (_, index) => sample(part, lerp(start, end, index / count)));
  });
}

export class Playback {
  constructor(total, durationSeconds) {
    if (!Number.isFinite(durationSeconds) || durationSeconds <= 0 || durationSeconds > 3600) {
      throw new Error('The replay duration is invalid.');
    }
    this.total = total;
    this.duration = durationSeconds;
    this.distance = 0;
    this.speed = 1;
    this.playing = false;
    this.mode = 'auto';
    this.followMode = 'auto';
    this.offsets = { heading: 0, pitch: 0, range: 1 };
  }
  play() {
    if (!this.total) return;
    if (this.distance >= this.total) this.distance = 0;
    this.playing = true;
  }
  pause() { this.playing = false; }
  seek(distance) {
    if (!Number.isFinite(distance)) return;
    this.pause();
    this.distance = clamp(distance, 0, this.total);
  }
  advance(seconds) {
    if (!this.playing || !Number.isFinite(seconds) || seconds < 0) return;
    this.distance = Math.min(this.total, this.distance + seconds * this.speed * this.total / this.duration);
    if (this.distance >= this.total) this.pause();
  }
  camera(mode) {
    if (mode === 'overview') {
      if (this.mode !== 'overview') this.followMode = this.mode;
      this.mode = 'overview';
    } else if (mode === 'follow') this.mode = this.followMode;
    else if (mode === 'auto') {
      this.mode = this.followMode = 'auto';
      this.offsets = { heading: 0, pitch: 0, range: 1 };
    }
  }
  cameraSnapshot() {
    return { mode: this.mode, followMode: this.followMode, offsets: { ...this.offsets } };
  }
  restoreCamera(snapshot) {
    if (!snapshot || !['auto', 'adjusted', 'overview'].includes(snapshot.mode) ||
        !['auto', 'adjusted'].includes(snapshot.followMode) ||
        (snapshot.mode !== 'overview' && snapshot.mode !== snapshot.followMode)) return false;
    const { heading, pitch, range } = snapshot.offsets ?? {};
    if (![heading, pitch, range].every(Number.isFinite) || Math.abs(heading) > Math.PI ||
        Math.abs(pitch) > Math.PI / 3 || range < 0.17 || range > 5) return false;
    this.mode = snapshot.mode;
    this.followMode = snapshot.followMode;
    this.offsets = { heading, pitch, range };
    return true;
  }
  adjust(heading, pitch, range) {
    this.offsets.heading = angleDelta(0, this.offsets.heading + heading);
    this.offsets.pitch = clamp(this.offsets.pitch + pitch, -Math.PI / 3, Math.PI / 3);
    this.offsets.range = clamp(this.offsets.range * range, 0.17, 5);
    this.mode = this.followMode = 'adjusted';
  }
}
