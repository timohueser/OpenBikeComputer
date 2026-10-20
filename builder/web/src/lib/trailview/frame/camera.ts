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
/** The time constant in seconds with which the eye follows the probed ground; a larger step than JUMP is a new place. */
const FOLLOW_S = 0.12, JUMP = 30;

/** The keys held: each axis is -1, 0 or 1. */
export interface Steer { ahead: number; right: number; up: number; run: boolean }

/** The eye's height above sea level. A walking eye over an unknown ground has none: an unknown height is never 0 m. */
export function eyeAltitude(pose: Pose, ground: number | undefined): number | undefined {
    if (pose.mode === 'fly') return altitude(pose.height, ground);
    return ground === undefined ? undefined : ground + pose.height;
}

/**
 * The ground that the eye stands on. It follows the GPU probe, which comes one to three frames late,
 * smoothly, and takes a jump at once.
 */
export function follow(held: number | undefined, probe: number, dt: number): number {
    if (held === undefined || Math.abs(probe - held) > JUMP) return probe;
    const next = probe + (held - probe) * Math.exp(-dt / FOLLOW_S);
    return Math.abs(next - probe) < 1e-3 ? probe : next;
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
