/** The weather presets: the cloud layer, the wet and the rain, and what they do to the light. */

import * as THREE from 'three';
import type { Light } from './light';

export interface Weather {
  /** Share of the sky under cloud, 0–1. */
  cover: number;
  /** Cloud base in metres above sea level, a climate fact; drawn at least 200 m above the eye. */
  base: number;
  /** How dark a thick cloud's underside is, 0–1. */
  dark: number;
  /** Sunlight and sky light, scaled. */
  sun: number;
  ambient: number;
  /** How far the sky and the sky light turn to these greys, which keep the hour's brightness. */
  grey: number;
  zenith: string;
  horizon: string;
  /** Visibility, as a share of the look's haze distance. */
  haze: number;
  /** Wet ground, and rain over the view, 0–1. */
  wet: number;
  rain: number;
  /** Cumulus towering along the horizon, 0–1: a maritime summer's fair weather. */
  towers?: number;
}

export const WEATHERS: Record<string, Weather> = {
  sunny: { cover: 0.14, base: 2200, dark: 0.35, sun: 1, ambient: 1, grey: 0, zenith: '#7f8a96', horizon: '#b4bcc4', haze: 1, wet: 0, rain: 0 },
  cloudy: { cover: 0.62, base: 1500, dark: 0.6, sun: 0.95, ambient: 1.15, grey: 0.5, zenith: '#7f8a96', horizon: '#b4bcc4', haze: 0.7, wet: 0, rain: 0 },
  rainy: { cover: 0.97, base: 800, dark: 0.85, sun: 0.6, ambient: 0.9, grey: 0.9, zenith: '#59626c', horizon: '#878e95', haze: 0.3, wet: 1, rain: 1 },
};

const luminance = (c: THREE.Color) => 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;

/** A colour turned toward a grey's hue at its own brightness. */
function toward(colour: THREE.Color, grey: string, share: number): THREE.Color {
  const target = new THREE.Color(grey);
  return colour.clone().lerp(target.multiplyScalar(luminance(colour) / luminance(target)), share);
}

/** The light under a weather. The sun stays where it is; clouds take it per point in the shaders. */
export function weatherLight(light: Light, weather: Weather): Light {
  return {
    ...light,
    sunColour: light.sunColour.clone().multiplyScalar(weather.sun),
    zenith: toward(light.zenith, weather.zenith, weather.grey),
    horizon: toward(light.horizon, weather.horizon, weather.grey),
    ambient: toward(light.ambient, weather.horizon, weather.grey).multiplyScalar(weather.ambient),
    glow: light.glow * (1 - weather.grey),
  };
}
