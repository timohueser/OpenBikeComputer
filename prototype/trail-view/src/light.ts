/** The sun for a place, a day and a wall-clock hour, and the sky and light keyed by its altitude. */

import * as THREE from 'three';
import { instantAt, solarPosition, solarTerms } from 'planner-sun';

/** One day per season, in the year of the Sentinel-2 quarterly mosaics. */
export const SEASONS: Record<string, string> = {
  'winter (15 Feb)': '2025-02-15',
  'spring (15 May)': '2025-05-15',
  'summer (15 Aug)': '2025-08-15',
  'autumn (15 Nov)': '2025-11-15',
};

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

// Night, blue hour, alpenglow, golden hour and day. The sun keeps a red colour a few degrees
// below the eye's horizon, because high peaks still see it there. Sun plus sky light stay near 1,
// so a lit face does not wash out.
const KEYS: Key[] = [
  { altitude: -18, zenith: '#0b1322', horizon: '#1a2438', sun: '#000000', ambient: '#4a5f96', glow: 0 },
  { altitude: -9, zenith: '#1b2f52', horizon: '#4f5d8a', sun: '#000000', ambient: '#55689c', glow: 0.15 },
  { altitude: -4, zenith: '#2c4a7a', horizon: '#b98a98', sun: '#3a1010', ambient: '#5f6c9a', glow: 0.5 },
  { altitude: -1, zenith: '#3d5f93', horizon: '#f0a07c', sun: '#e05a40', ambient: '#646e98', glow: 0.9 },
  { altitude: 3, zenith: '#4a75ad', horizon: '#f5bc8a', sun: '#e8904f', ambient: '#5f6d96', glow: 0.8 },
  { altitude: 10, zenith: '#5486c4', horizon: '#eed6b4', sun: '#e6c08e', ambient: '#5b6d94', glow: 0.45 },
  { altitude: 25, zenith: '#5a8fd0', horizon: '#c8dbeb', sun: '#dcd2bd', ambient: '#5a6f94', glow: 0.2 },
  { altitude: 60, zenith: '#4f88d2', horizon: '#bcd4ea', sun: '#ddd6c6', ambient: '#5a6f94', glow: 0.1 },
];

const mix = (a: string, b: string, t: number) => new THREE.Color(a).lerp(new THREE.Color(b), t);

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
    sunColour: mix(a.sun, b.sun, t),
    zenith: mix(a.zenith, b.zenith, t),
    horizon: mix(a.horizon, b.horizon, t),
    glow: a.glow + (b.glow - a.glow) * t,
    ambient: mix(a.ambient, b.ambient, t),
    night: Math.min(Math.max((-4 - degrees) / 8, 0), 1),
  };
}
