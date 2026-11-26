// A tree's trunk: its bark, marks, flutes or diamond leaf scars.

import { computeNormals, hex, lathe, nonIndexed, paint, random, sheet, smooth, soft, v3, type Colour, type Geometry, type V } from '../../shape';
import type { Bark, Lod } from '../../trees';
import { DETAIL, lerp } from './parts';

/** Bands of marks up a near marked trunk; each holds up to two dashes. */
const BANDS = 18;

/** A trunk from the ground to `top`, darker at its foot, its bole in `spans` spans, which a trunk that bends needs. */
export function trunk({ trunk: r, flare, neck = 0.6, bark, fade = [0, 0.5], marks, scars, flutes, twist }: Bark, top: number, lod: Lod, seed: number, spans = 1): Geometry {
    if (scars && lod === 0) return scarred(r, flare, neck, bark, scars, top);
    const [sides, perSpan] = DETAIL[lod].trunk;
    const [foot, bole] = bark.map(hex);
    const low = Math.min(6 * r, 0.5 * top);
    const at = (z: number): [number, number] => [r * lerp(1, neck, (z - low) / (top - low)), z];
    // A near marked trunk has a row at each edge of each band of marks, which are rows enough.
    const bands = marks && lod === 0 ? BANDS : 0;
    const centre = (b: number) => low + ((b + 0.5) / bands) * (top - low);
    const half = 0.3 * r;
    const profile: [number, number][] = [
        [0, 0],
        [r * flare, 0],
        // A far trunk's flare is one straight span.
        ...(lod < 2 ? [[(r * (1 + flare)) / 2, 2 * r] as [number, number]] : []),
        ...(bands ? Array.from({ length: bands }, (_, b) => [at(centre(b) - half), at(centre(b) + half)]).flat() : Array.from({ length: spans }, (_, k) => at(lerp(low, top, k / spans)))),
        at(top),
        [0, top + r],
    ];
    // The lathe samples its spline evenly per span, so whole rows per span keep every profile point.
    const geo = soft(lathe(profile, sides, (bands ? 1 : perSpan) * (profile.length - 1)), 0.075 * r, 1.4 / r, seed);
    if (flutes) {
        const p = geo.position;
        for (let i = 0; i < p.length; i += 3) {
            // A side stands out and the next stands in, less the higher up.
            const k = 1 + 0.3 * Math.cos(flutes * (Math.atan2(p[i + 1], p[i]) + Math.PI / 2 + (2 * Math.PI * (twist ?? 0) * p[i + 2]) / top)) * smooth(twist ? 1.5 * top : 0.5 * top, 0, p[i + 2]);
            p[i] *= k;
            p[i + 1] *= k;
        }
        computeNormals(geo);
    }
    // A far scarred trunk takes the mean shade of its plates and grooves.
    const colour = (p: V) => foot.clone().lerp(bole, smooth(fade[0] * top, fade[1] * top, p.z)).multiplyScalar(scars ? 0.8 : 1);
    if (!marks) return paint(geo, colour);
    const mark = hex(marks[0]);
    if (!bands) return paint(geo, (p) => colour(p).lerp(mark, 0.3 * marks[1]));
    // Dashes an eighth to a quarter of the way round.
    const rand = random(seed);
    const dashes = Array.from({ length: bands }, () => [0, 1].flatMap(() => (rand() < marks[1] ? [[2 * Math.PI * rand(), (Math.PI / 8) * (1 + rand())]] : [])));
    return stain(paint(geo, colour), mark, (c) => {
        const b = Math.round(((c.z - low) / (top - low)) * bands - 0.5);
        const a = Math.atan2(c.y, c.x);
        return Math.abs(c.z - centre(b)) < half && (dashes[b] ?? []).some(([to, width]) => Math.abs(((a - to + 3 * Math.PI) % (2 * Math.PI)) - Math.PI) < width);
    });
}

/** Paints whole faces whose centre `dark` picks, so marks keep the sharp edges vertex colours would blur. */
function stain(geo: Geometry, mark: Colour, dark: (centre: V) => boolean): Geometry {
    const flat = nonIndexed(geo), p = flat.position, c = v3();
    for (let i = 0; i < p.length / 3; i += 3) {
        c.set(p[3 * i] + p[3 * i + 3] + p[3 * i + 6], p[3 * i + 1] + p[3 * i + 4] + p[3 * i + 7], p[3 * i + 2] + p[3 * i + 5] + p[3 * i + 8]).divideScalar(3);
        if (dark(c)) for (let j = i; j < i + 3; j++) flat.color.splice(3 * j, 3, mark.r, mark.g, mark.b);
    }
    return flat;
}

/**
 * A trunk of diamond leaf scars in rings, each ring turned half a diamond. A diamond is four flat
 * faces around a raised centre, so the sun lights its upper half and the grooves stay dark:
 * 4 × n triangles a ring. The trunk swells to `neck` radii under the crown.
 */
function scarred(r: number, flare: number, neck: number, bark: [string, string], n: number, top: number): Geometry {
    const [foot, bole] = bark.map(hex);
    // Rings half a diamond apart; a diamond is half as tall as it is wide.
    const h = (0.25 * 2 * Math.PI * r) / n;
    const corner = (i: number, k: number, raised = false) => {
        const z = i * h;
        // Scars run in a slow spiral, as the leaves grew.
        const a = (k / 2 + 0.03 * i) * ((2 * Math.PI) / n);
        const radius = r * (1 + (flare - 1) * (1 - smooth(0, 4 * r, z))) * lerp(1, neck, smooth(0.4 * top, top, z)) * (raised ? 1.15 : 1);
        return v3(radius * Math.cos(a), radius * Math.sin(a), z);
    };
    const cards = sheet();
    for (let i = 0; i * h < top; i++) {
        for (let k = i % 2; k < 2 * n; k += 2) {
            const c = corner(i, k, true);
            const ring = [corner(i, k + 1), corner(i + 1, k), corner(i, k - 1), corner(i - 1, k)];
            const plate = foot.clone().lerp(bole, smooth(0, 0.5 * top, c.z));
            const groove = plate.clone().multiplyScalar(0.55);
            for (let e = 0; e < 4; e++) cards.add([c, ring[e], ring[(e + 1) % 4]], [plate, groove, groove]);
        }
    }
    return cards.build();
}
