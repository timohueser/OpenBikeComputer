/** Walk and fly, and the camera's projection. Angles are degrees: heading clockwise from north, pitch up. */

export type Mode = 'walk' | 'fly';

/** `height` is the eye above the ground when walking, and the altitude above sea level when flying. */
export interface Pose { east: number; north: number; heading: number; pitch: number; mode: Mode; height: number }

export const WALK_EYE = 1.6;
export const MAX_ALTITUDE = 100_000;
/** The lowest a flying eye goes above the ground. */
export const MIN_CLEARANCE = 2;
export const VFOV = 60;
export const NEAR = 0.1;
const WALK_M_S = 6, RUN = 12;
const RAD = Math.PI / 180;
/** The time constant in seconds with which a step between two probes fades out; a larger step than JUMP is a new place. */
const FOLLOW_S = 0.12, JUMP = 30;

/** The keys held: each axis is -1, 0 or 1. */
export interface Steer { ahead: number; right: number; up: number; run: boolean }

/** The eye's height above sea level. A walking eye over an unknown ground has none: an unknown height is never 0 m. */
export function eyeAltitude(pose: Pose, ground: number | undefined): number | undefined {
    if (pose.mode === 'fly') return altitude(pose.height, ground);
    return ground === undefined ? undefined : ground + pose.height;
}

/** The GPU probe of the ground at a point of the frame: its height, its slope (metres per metre east and north) and its frame. */
export interface Probe { east: number; north: number; height: number; slope: [number, number]; frame: number }

/** The ground that the eye stands on: the plane of a probe, and a step that fades out. */
export interface Footing { probe: Probe; step: number }

export function groundOf({ probe, step }: Footing, east: number, north: number): number {
    return probe.height + probe.slope[0] * (east - probe.east) + probe.slope[1] * (north - probe.north) + step;
}

/**
 * The footing on the last probe, which comes one to three frames late. The eye moves on the probe's
 * plane, so a slope gives no lag. Where a new probe disagrees with the one before, at a finer page or
 * a bend of the ground, the step between them fades out; a step larger than JUMP is a new place.
 */
export function follow(footing: Footing | undefined, probe: Probe, east: number, north: number, dt: number): Footing {
    if (!footing) return { probe, step: 0 };
    let step = footing.step * Math.exp(-dt / FOLLOW_S);
    if (probe.frame !== footing.probe.frame) step += groundOf({ probe: footing.probe, step: 0 }, east, north) - groundOf({ probe, step: 0 }, east, north);
    return { probe, step: Math.abs(step) > JUMP || Math.abs(step) < 1e-3 ? 0 : step };
}

/** One step of free movement. A flying eye moves faster the higher it is above the ground. */
export function move(pose: Pose, steer: Steer, dt: number, ground: number | undefined): Pose {
    const z = eyeAltitude(pose, ground);
    const above = z === undefined || ground === undefined ? pose.height : z - ground;
    const speed = (pose.mode === 'walk' ? WALK_M_S : Math.max(20, above)) * (steer.run ? RUN : 1) * dt;
    const h = pose.heading * RAD;
    const east = pose.east + (Math.sin(h) * steer.ahead + Math.cos(h) * steer.right) * speed;
    const north = pose.north + (Math.cos(h) * steer.ahead - Math.sin(h) * steer.right) * speed;
    const height = pose.mode === 'fly' ? altitude(pose.height + steer.up * speed, ground) : pose.height;
    return { ...pose, east, north, height };
}

/** The wheel: a walking eye takes off, a flying one rises or sinks by a share of its height above the ground. */
export function zoom(pose: Pose, deltaY: number, ground: number | undefined): Pose {
    const z = eyeAltitude(pose, ground), share = Math.exp(deltaY * 0.002);
    if (z === undefined) return pose;
    return { ...pose, mode: 'fly', height: altitude(ground === undefined ? z * share : ground + (z - ground) * share, ground) };
}

/** Walk on the ground under the eye, or fly from where the eye is. */
export function toggle(pose: Pose, ground: number | undefined): Pose {
    if (pose.mode === 'fly') return { ...pose, mode: 'walk', height: WALK_EYE };
    const z = eyeAltitude(pose, ground);
    return z === undefined ? pose : { ...pose, mode: 'fly', height: z };
}

export function look(pose: Pose, dx: number, dy: number, degreesPerPixel: number): Pose {
    const heading = ((pose.heading - dx * degreesPerPixel) % 360 + 360) % 360;
    return { ...pose, heading, pitch: Math.min(89, Math.max(-89, pose.pitch + dy * degreesPerPixel)) };
}

/** An altitude in range: at most MAX_ALTITUDE, and MIN_CLEARANCE above a known ground. */
const altitude = (value: number, ground: number | undefined) => Math.min(MAX_ALTITUDE, ground === undefined ? value : Math.max(ground + MIN_CLEARANCE, value));

/**
 * Camera-relative world (x east, y north, z up) to clip space, column-major. Reversed Z with an
 * infinite far plane: depth is NEAR / distance, 1 at the near plane and 0 at infinity.
 */
export function viewProjection(heading: number, pitch: number, aspect: number): Float32Array {
    const h = heading * RAD, p = pitch * RAD, f = 1 / Math.tan(VFOV * RAD / 2);
    const forward = [Math.sin(h) * Math.cos(p), Math.cos(h) * Math.cos(p), Math.sin(p)];
    const right = [Math.cos(h), -Math.sin(h), 0];
    const up = [
        right[1] * forward[2] - right[2] * forward[1],
        right[2] * forward[0] - right[0] * forward[2],
        right[0] * forward[1] - right[1] * forward[0],
    ];
    const m = new Float32Array(16);
    for (let j = 0; j < 3; j++) m.set([f / aspect * right[j], f * up[j], 0, forward[j]], 4 * j);
    m.set([0, 0, NEAR, 0], 12);
    return m;
}
