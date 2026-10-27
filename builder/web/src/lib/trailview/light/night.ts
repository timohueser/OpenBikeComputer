/**
 * How the night sky shows: the sky's brightness at the zenith from the place's own sky, the twilight
 * and the moon, the faintest star that an eye sees against it, and the Milky Way's share. Magnitudes
 * are V; sky brightness is in magnitudes per square arcsecond, as a sky quality meter reads it.
 */

/**
 * The sky brightness of a moonless night far from lights, which the engine takes until a place
 * gives its own (`TrailView.skyBrightness`): a rural sky, a little brighter than the natural 22.0.
 */
export const DARK_SKY = 21.8;

/**
 * The zenith by the sun's altitude in degrees: about a magnitude a degree from −6° to −12° (Patat et
 * al. 2006, rounded). The day's keys are chosen, near 3000 cd/m² high up and 300 cd/m² at sunset.
 */
const TWILIGHT: [number, number][] = [[20, 4], [0, 6.5], [-4, 11], [-6, 13], [-9, 16.3], [-12, 19.3], [-15, 21.3], [-18, 23]];
/** The zenith in totality, lit by the air beyond the moon's shadow: chosen, so that about a dozen stars show, as observers report. */
const UMBRA = 14.5;
/** The zenith under a full moon high in a dark sky; a moon of the magnitude m makes it m − FULL_MOON brighter. */
const MOONLIT_SKY = 17.6, FULL_MOON = -12.73;
/** The Milky Way fades out of a sky between these brightnesses. */
const MILKY_WAY: [number, number] = [19, 21.5];

export interface NightInput {
    /** The sun's and the moon's altitudes in degrees. */
    sun: number;
    moon: number;
    /** The moon's phase angle in degrees: 0 full, 180 new. */
    phase: number;
    /** The place's own sky without twilight and moon, magnitudes per square arcsecond. */
    sky: number;
    /** The cloud cover of the weather, 0 to 1. */
    cover: number;
    /** The share of the sun's light that an eclipse leaves, 1 without one. */
    left: number;
}

export interface NightSky {
    /** The sky at the zenith, magnitudes per square arcsecond. */
    zenith: number;
    /** The faintest star that shows. */
    limit: number;
    /** The share of the stars' light that the clouds let through. */
    stars: number;
    /** The Milky Way's share, 0 to 1. */
    milkyWay: number;
}

const flux = (magnitude: number) => 10 ** (-0.4 * magnitude);
const smoothstep = (a: number, b: number, x: number) => {
    const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
    return t * t * (3 - 2 * t);
};

/** The moon's magnitude at a phase angle in degrees (Allen). */
export const moonMagnitude = (phase: number) => FULL_MOON + 0.026 * Math.abs(phase) + 4e-9 * phase ** 4;

/** The night sky at a place and an instant. */
export function nightSky({ sun, moon, phase, sky, cover, left }: NightInput): NightSky {
    const next = TWILIGHT.findIndex(([altitude]) => altitude < sun), below = next < 0 ? TWILIGHT.length - 1 : Math.max(next, 1);
    const [a, b] = [TWILIGHT[below - 1], TWILIGHT[below]];
    const twilight = a[1] + (b[1] - a[1]) * Math.min(Math.max((sun - a[0]) / (b[0] - a[0]), 0), 1);
    // The sunlit air dims with the sun's light, down to the light of the air beyond the shadow.
    const sunlit = flux(twilight) * left + flux(UMBRA) * (1 - left);
    const up = Math.sin(Math.max(moon, 0) * Math.PI / 180);
    const moonlit = flux(MOONLIT_SKY + moonMagnitude(phase) - FULL_MOON) * smoothstep(-1, 5, moon) * (0.4 + 0.6 * up);
    const zenith = -2.5 * Math.log10(flux(sky) + sunlit + moonlit);
    // The naked eye's limit against a sky (Schaefer 1990, as the Unihedron sky quality meter tables give it).
    const limit = 7.93 - 5 * Math.log10(10 ** (4.316 - zenith / 5) + 1);
    return { zenith, limit, stars: 1 - cover, milkyWay: smoothstep(...MILKY_WAY, zenith) * (1 - cover) };
}
