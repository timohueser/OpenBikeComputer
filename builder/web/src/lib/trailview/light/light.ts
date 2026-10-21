/**
 * The light keys by the sun's altitude, the weather presets and what they do to the light, and the
 * look of the regions at the eye. A port of the prototype's `light.ts` and `weather.ts`. Colours
 * are linear RGB.
 */

import { linear } from '../ground/rules';
import type { Look, Preset, Region, WeatherKeys } from '../rules/regions';

type Rgb = [number, number, number];

export interface Light {
    sunColour: Rgb;
    zenith: Rgb;
    horizon: Rgb;
    /** Strength of the warm glow around the sun near the horizon. */
    glow: number;
    /** Sky light on a face that points straight up. */
    ambient: Rgb;
    /** 0 by day, 1 once the sun is 12° down: colours shift to blue. */
    night: number;
}

// Night, blue hour, alpenglow, golden hour and day. The sun keeps a red colour a few degrees below
// the eye's horizon, because high peaks still see it there. By day sun plus sky light stay near 1,
// so a lit face does not wash out, and shade keeps about a third of it; toward sunset the sky light
// drops faster than the sun, so contrast rises.
const KEYS = [
    { altitude: -18, zenith: '#0b1322', horizon: '#1a2438', sun: '#000000', ambient: '#2a3866', glow: 0 },
    { altitude: -9, zenith: '#1b2f52', horizon: '#4f5d8a', sun: '#000000', ambient: '#3a4878', glow: 0.15 },
    { altitude: -4, zenith: '#2c4a7a', horizon: '#c08a98', sun: '#401010', ambient: '#4a5482', glow: 0.6 },
    { altitude: -1, zenith: '#3d5f93', horizon: '#f39a72', sun: '#e2563a', ambient: '#5a5f8c', glow: 1.1 },
    { altitude: 3, zenith: '#4a75ad', horizon: '#f6b882', sun: '#ec8e48', ambient: '#68709a', glow: 1.0 },
    { altitude: 10, zenith: '#5486c4', horizon: '#efd2a8', sun: '#eebc84', ambient: '#7a86a6', glow: 0.55 },
    { altitude: 25, zenith: '#5a8fd0', horizon: '#c8dbeb', sun: '#dcd2bd', ambient: '#8795b0', glow: 0.2 },
    { altitude: 60, zenith: '#4f88d2', horizon: '#bcd4ea', sun: '#ddd6c6', ambient: '#8a98b4', glow: 0.1 },
];

// Sun and sky light on a face: the palette holds display colours, and a face in full daylight shows
// a little above its own colour.
const DAYLIGHT = 1.3;

const mix = (a: number[], b: number[], t: number) => a.map((v, i) => v + (b[i] - v) * t) as Rgb;
const scale = (c: number[], s: number) => c.map(v => v * s) as Rgb;

/** The light for a sun altitude in degrees. */
export function lightAt(degrees: number): Light {
    const above = KEYS.findIndex(key => key.altitude > degrees);
    const upper = above === -1 ? KEYS.length - 1 : Math.max(above, 1), a = KEYS[upper - 1], b = KEYS[upper];
    const t = Math.min(Math.max((degrees - a.altitude) / (b.altitude - a.altitude), 0), 1);
    const key = (name: 'sun' | 'zenith' | 'horizon' | 'ambient') => mix(linear(a[name]), linear(b[name]), t);
    return {
        sunColour: scale(key('sun'), DAYLIGHT), zenith: key('zenith'), horizon: key('horizon'),
        glow: a.glow + (b.glow - a.glow) * t, ambient: scale(key('ambient'), DAYLIGHT), night: Math.min(Math.max((-4 - degrees) / 8, 0), 1),
    };
}

/** The weather presets: the cloud layer, the wet and the rain, and what they do to the light. */
export const WEATHERS: Record<Preset, WeatherKeys> = {
    sunny: { cover: 0.14, base: 2200, dark: 0.35, sun: 1, ambient: 1, grey: 0, zenith: '#7f8a96', horizon: '#b4bcc4', haze: 1, wet: 0, rain: 0 },
    cloudy: { cover: 0.62, base: 1500, dark: 0.6, sun: 0.95, ambient: 1.15, grey: 0.5, zenith: '#7f8a96', horizon: '#b4bcc4', haze: 0.7, wet: 0, rain: 0 },
    rainy: { cover: 0.97, base: 800, dark: 0.85, sun: 0.6, ambient: 0.9, grey: 0.9, zenith: '#59626c', horizon: '#878e95', haze: 0.3, wet: 1, rain: 1 },
};

/** The weather of the eye in linear light: a preset's keys with the colours read. */
export interface Weather extends Omit<WeatherKeys, 'zenith' | 'horizon'> { zenith: Rgb; horizon: Rgb }

const luminance = (c: number[]) => 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];

/** A colour turned toward a grey's hue at its own brightness. */
const toward = (colour: Rgb, grey: Rgb, share: number) => mix(colour, scale(grey, luminance(colour) / luminance(grey)), share);

/** The light under a weather. The sun stays where it is. */
export function weatherLight(light: Light, weather: Weather): Light {
    return {
        ...light,
        sunColour: scale(light.sunColour, weather.sun),
        zenith: toward(light.zenith, weather.zenith, weather.grey),
        horizon: toward(light.horizon, weather.horizon, weather.grey),
        ambient: scale(toward(light.ambient, weather.horizon, weather.grey), weather.ambient),
        glow: light.glow * (1 - weather.grey),
    };
}

/** What applies where the eye is: haze, mist, light, grade and weather. */
export interface EyeLook {
    hazeKm: number; hazeHeight: number; mistTop: number; softness: number; gradient: number;
    shadow: Rgb; light: Rgb; saturation: number; weather: Weather;
}

/** The look at the eye: the looks of the regions about it, blended by their weights, which sum to 1. */
export function eyeLook(regions: Region[], weights: Map<number, number>, preset: Preset): EyeLook {
    let out: EyeLook | undefined;
    for (const [id, w] of weights) {
        const look: Look = regions[id].look, keys = { ...WEATHERS[preset], ...look.weather?.[preset] };
        const own: EyeLook = {
            hazeKm: look.hazeKm, hazeHeight: look.hazeHeight, mistTop: look.mistTop, softness: look.softness, gradient: look.gradient,
            shadow: look.grade.shadow, light: look.grade.light, saturation: look.grade.saturation,
            weather: { ...keys, towers: keys.towers ?? 0, zenith: linear(keys.zenith), horizon: linear(keys.horizon) },
        };
        out = added(out, own, w);
    }
    if (!out) throw new Error('The look at the eye needs the weight of a region.');
    return out;
}

/** `sum` plus w × `value`, number by number; no sum is 0. */
function added<T>(sum: T | undefined, value: T, w: number): T {
    if (typeof value === 'number') return (((sum as number | undefined) ?? 0) + value * w) as T;
    if (Array.isArray(value)) return value.map((v, i) => ((sum as number[] | undefined)?.[i] ?? 0) + v * w) as T;
    return Object.fromEntries(Object.entries(value as object).map(([key, v]) => [key, added((sum as Record<string, unknown> | undefined)?.[key], v, w)])) as T;
}
