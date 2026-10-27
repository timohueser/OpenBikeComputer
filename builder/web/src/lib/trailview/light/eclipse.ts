/**
 * A solar eclipse as the sky draws it: the share of the sun's light that the moon leaves, the moon
 * drawn at the sun's scale about the sun, and the corona's look. Nothing lists eclipses: the moon
 * covers the sun wherever `celestial.ts` puts the two together.
 */

import type { Body } from './celestial';

type Vec3 = [number, number, number];

/** The drawn sun's half width in radians: about twice the real one. */
export const SUN_DISK = 0.0085;
/** The sun's limb darkening: the light at the limb against the centre is 1 − u. */
const LIMB = 0.6;
/** Rings of the sun's disk that `sunlightLeft` sums. */
const RINGS = 48;
/** The moon draws at the sun's scale about the sun closer than the first angle, and on its own beyond the second. */
const NEAR = 2 * Math.PI / 180, FAR = 4 * Math.PI / 180;
/** The share of the sun's light at which the corona shows half: it is about a millionth of the sun's light, the sky's light falls with the sun's. */
const CORONA = 3e-4;
/** A minimum of the sun's eleven-year cycle (December 2019), and the cycle's mean length in years. */
const MINIMUM = Date.UTC(2019, 11, 15), CYCLE = 11;
/** A new moon (6 January 2000, 18:14 UTC) and the synodic month in days: an eclipse's lunation. */
const NEW_MOON = Date.UTC(2000, 0, 6, 18, 14), MONTH = 29.530589;

const dot = (a: number[], b: number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const smoothstep = (a: number, b: number, x: number) => {
    const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
    return t * t * (3 - 2 * t);
};

/** The angle between two directions, exact also where they almost agree. */
function separation(a: number[], b: number[]): number {
    const cross = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    return Math.atan2(Math.hypot(...cross), dot(a, b));
}

/**
 * The share of the sun's light that the moon leaves: 1 without an eclipse, 0 in totality. It sums
 * rings of the sun's disk, each darkened toward the limb, so the last crescent gives little light.
 */
export function sunlightLeft(sun: Body, moon: Body): number {
    const d = separation(sun.toward, moon.toward) / sun.radius, r = moon.radius / sun.radius;
    if (d >= 1 + r) return 1;
    if (d + 1 <= r) return 0;
    let left = 0, whole = 0;
    for (let i = 0; i < RINGS; i++) {
        // The rings crowd toward the limb, where the last crescent lies: ρ = 1 − (1 − x)².
        const x = (i + 0.5) / RINGS, rho = 1 - (1 - x) ** 2, weight = rho * (1 - x) * (1 - LIMB * (1 - Math.sqrt(1 - rho * rho)));
        // The share of the ring's circle outside the moon: the moon covers the arc of half angle α about it.
        const outside = rho + d <= r ? 0 : d >= rho + r || r + d <= rho ? 1 : 1 - Math.acos((rho * rho + d * d - r * r) / (2 * rho * d)) / Math.PI;
        left += weight * outside;
        whole += weight;
    }
    return left / whole;
}

/**
 * The moon as the sky draws it: 1.5 times its size high in the sky, growing to 3 times at the
 * horizon as the eye sees it. Near the sun it draws at the drawn sun's scale, its offset from the sun
 * scaled alike, so an eclipse keeps its shape. The blend happens near a new moon, which no eye sees.
 */
export function drawnMoon(sun: Body, moon: Body): { toward: Vec3; radius: number } {
    const low = 1 - Math.max(moon.toward[2], 0), alone = moon.radius * (1.5 + 1.5 * low * low), scale = SUN_DISK / sun.radius;
    const t = smoothstep(FAR, NEAR, separation(sun.toward, moon.toward));
    const at = moon.toward.map((v, i) => v + t * (sun.toward[i] + scale * (v - sun.toward[i]) - v)), length = Math.hypot(...at);
    return { toward: at.map(v => v / length) as Vec3, radius: alone + t * (scale * moon.radius - alone) };
}

/** How much of the corona shows while an eclipse leaves the share `left` of the sun's light: 0 without an eclipse, 1 in totality. */
export function coronaShare(left: number): number {
    return (1 - left) / (1 + left / CORONA);
}

/** The sun's activity at an instant: 0 at a minimum of its cycle, 1 at a maximum. A round corona at a maximum, a corona drawn out along the sun's equator at a minimum. */
export function solarActivity(instant: number): number {
    return 0.5 - 0.5 * Math.cos(2 * Math.PI * (instant - MINIMUM) / (CYCLE * 365.25 * 86400000));
}

/** A number in [0, 1) that stays the same through an eclipse and differs from one eclipse to the next: the seed of its corona's streamers. */
export function coronaSeed(instant: number): number {
    const lunation = Math.round((instant - NEW_MOON) / (MONTH * 86400000));
    return ((lunation * 0.6180340) % 1 + 1) % 1;
}
