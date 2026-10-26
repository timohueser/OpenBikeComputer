import { linear, VARIEGATED } from '../ground/rules';
import type { EyeLook, Light } from './light';

/** Bytes of `Look` in shade.wgsl, and the offset of its `codes`. */
export const LOOK_BYTES = 288, LOOK_CODES = 240;

export interface LookInput {
    /** Toward the sun in render space, and its bearing in turns clockwise from true north. */
    sun: number[];
    light: Light;
    eye: EyeLook;
    /** The eye: metres above sea level, and metres east and north in the frame. */
    altitude: number;
    place: [number, number];
    /** The view ray of clip point (x, y) is forward + x right + y up. */
    camera: { right: number[]; up: number[]; forward: number[] };
    /** The material code of heath, the finest ground level, and the materials and lithologies of the vocabulary. */
    codes: [number, number, number, number];
    debug: number;
    /** The jittered frame's sample. */
    sample: number;
    /** This frame's jitter in clip space, and the render scale. */
    jitter: [number, number];
    scale: number;
}

/** `Look` of shade.wgsl. */
export function lookUniform(input: LookInput): ArrayBuffer {
    const data = new ArrayBuffer(LOOK_BYTES), floats = new Float32Array(data), words = new Uint32Array(data);
    const { light, eye } = input;
    floats.set(input.sun, 0);
    floats.set([...light.sunColour, light.glow], 4);
    floats.set([...light.ambient, light.night], 8);
    floats.set([...light.zenith, eye.softness], 12);
    floats.set([...light.horizon, eye.gradient], 16);
    floats.set([...eye.shadow, eye.saturation], 20);
    floats.set([...eye.light, eye.weather.wet], 24);
    floats.set([eye.hazeKm * 1000 * eye.weather.haze, eye.hazeHeight, eye.mistTop, input.altitude], 28);
    floats.set([...input.place, 0, 0], 32);
    VARIEGATED.forEach((hex, i) => floats.set([...linear(hex), 0], 36 + 4 * i));
    floats.set([...input.camera.right, 0, ...input.camera.up, 0, ...input.camera.forward, 0], 48);
    words.set(input.codes, LOOK_CODES / 4);
    words.set([input.debug, input.sample, 0, 0], 64);
    floats.set([...input.jitter, input.scale, 0], 68);
    return data;
}
