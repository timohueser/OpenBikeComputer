/** The sun for a place, a day and a wall-clock hour, and the sky and light keyed by its altitude. */

import * as THREE from 'three';
import { instantAt, solarPosition, solarTerms } from 'planner-sun';

export interface Light {
  /** Towards the sun, in the eye's east-north-up frame. */
  sun: THREE.Vector3;
  altitude: number;
  sunColour: THREE.Color;
  zenith: THREE.Color;
  horizon: THREE.Color;
  /** Strength of the warm glow around the sun near the horizon. */
  glow: number;
  /** Sky light on a face that points straight up. */
  ambient: THREE.Color;
  /** 0 by day, 1 once the sun is 12° down: colours shift to blue. */
  night: number;
}

interface Key {
  altitude: number;
  zenith: string;
  horizon: string;
  sun: string;
  /** Sky light on an upward face. It stays blue and visible at night. */
  ambient: string;
  glow: number;
}

// Night, blue hour, alpenglow, golden hour and day, in linear light once read. The sun keeps a
// red colour a few degrees below the eye's horizon, because high peaks still see it there. By
// day sun plus sky light stay near 1, so a lit face does not wash out, and shade keeps about a
// third of it; toward sunset the sky light drops faster than the sun, so contrast rises.
const KEYS: Key[] = [
  { altitude: -18, zenith: '#0b1322', horizon: '#1a2438', sun: '#000000', ambient: '#2a3866', glow: 0 },
  { altitude: -9, zenith: '#1b2f52', horizon: '#4f5d8a', sun: '#000000', ambient: '#3a4878', glow: 0.15 },
  { altitude: -4, zenith: '#2c4a7a', horizon: '#c08a98', sun: '#401010', ambient: '#4a5482', glow: 0.6 },
  { altitude: -1, zenith: '#3d5f93', horizon: '#f39a72', sun: '#e2563a', ambient: '#5a5f8c', glow: 1.1 },
  { altitude: 3, zenith: '#4a75ad', horizon: '#f6b882', sun: '#ec8e48', ambient: '#68709a', glow: 1.0 },
  { altitude: 10, zenith: '#5486c4', horizon: '#efd2a8', sun: '#eebc84', ambient: '#7a86a6', glow: 0.55 },
  { altitude: 25, zenith: '#5a8fd0', horizon: '#c8dbeb', sun: '#dcd2bd', ambient: '#8795b0', glow: 0.2 },
  { altitude: 60, zenith: '#4f88d2', horizon: '#bcd4ea', sun: '#ddd6c6', ambient: '#8a98b4', glow: 0.1 },
];

const mix = (a: string, b: string, t: number) => new THREE.Color(a).lerp(new THREE.Color(b), t);

// Sun and sky light on a face, over the keys: the palette holds display colours, and a face in
// full daylight shows a little above its own colour.
const DAYLIGHT = 1.3;

/** The light at a place and time. `hour` is local wall-clock time in `timezone`. */
export function lightAt(lat: number, lon: number, date: string, hour: number, timezone: string): Light {
  const instant = instantAt(date, Math.round(hour * 60), timezone);
  const { altitude, bearing } = solarPosition([lon, lat], solarTerms(instant));
  const sun = new THREE.Vector3(
    Math.sin(bearing) * Math.cos(altitude),
    Math.cos(bearing) * Math.cos(altitude),
    Math.sin(altitude),
  );
  const degrees = (altitude * 180) / Math.PI;
  const above = KEYS.findIndex((key) => key.altitude > degrees);
  const upper = above === -1 ? KEYS.length - 1 : Math.max(above, 1);
  const [a, b] = [KEYS[upper - 1], KEYS[upper]];
  const t = Math.min(Math.max((degrees - a.altitude) / (b.altitude - a.altitude), 0), 1);
  return {
    sun,
    altitude: degrees,
    sunColour: mix(a.sun, b.sun, t).multiplyScalar(DAYLIGHT),
    zenith: mix(a.zenith, b.zenith, t),
    horizon: mix(a.horizon, b.horizon, t),
    glow: a.glow + (b.glow - a.glow) * t,
    ambient: mix(a.ambient, b.ambient, t).multiplyScalar(DAYLIGHT),
    night: Math.min(Math.max((-4 - degrees) / 8, 0), 1),
  };
}
