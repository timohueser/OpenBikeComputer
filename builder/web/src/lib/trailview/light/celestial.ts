/**
 * The sky about a place at an instant: the rotation from the equator of the stars (J2000) to east,
 * north and up, and the moon there. The moon is the series of Meeus, Astronomical Algorithms,
 * chapter 47, cut to its largest terms: about 2′ off, against the parallax of up to 1° that the
 * topocentric place corrects.
 */

/** A rotation, rows first. */
export type Mat3 = [number, number, number, number, number, number, number, number, number];
type Vec3 = [number, number, number];

const RAD = Math.PI / 180;
/** The Earth's equatorial radius in km. */
const EARTH_KM = 6378.14;

const times = (m: Mat3, [x, y, z]: Vec3): Vec3 => [m[0] * x + m[1] * y + m[2] * z, m[3] * x + m[4] * y + m[5] * z, m[6] * x + m[7] * y + m[8] * z];
const product = (a: Mat3, b: Mat3): Mat3 => [0, 1, 2].flatMap(i => [0, 1, 2].map(j => a[3 * i] * b[j] + a[3 * i + 1] * b[3 + j] + a[3 * i + 2] * b[6 + j])) as Mat3;
const aboutZ = (a: number): Mat3 => [Math.cos(a), -Math.sin(a), 0, Math.sin(a), Math.cos(a), 0, 0, 0, 1];
const aboutY = (a: number): Mat3 => [Math.cos(a), 0, -Math.sin(a), 0, 1, 0, Math.sin(a), 0, Math.cos(a)];

/** Days and Julian centuries from J2000.0. The engine takes UTC for TT: the moon moves 0.01° in their 69 s. */
function epoch(instant: number): { d: number; t: number } {
    const d = instant / 86400000 + 2440587.5 - 2451545;
    return { d, t: d / 36525 };
}

/** Equatorial coordinates of the date to east, north and up at a place: by the sidereal time and the latitude. */
function horizon([lon, lat]: [number, number], instant: number): Mat3 {
    const { d, t } = epoch(instant), sidereal = (280.46061837 + 360.98564736629 * d + 0.000387933 * t * t + lon) * RAD, phi = lat * RAD;
    const [s, c] = [Math.sin(sidereal), Math.cos(sidereal)];
    return [-s, c, 0, -Math.sin(phi) * c, -Math.sin(phi) * s, Math.cos(phi), Math.cos(phi) * c, Math.cos(phi) * s, Math.sin(phi)];
}

/** J2000 equatorial coordinates to east, north and up: the precession to the date (IAU 1976), then the horizon. */
export function starFrame(place: [number, number], instant: number): Mat3 {
    const { t } = epoch(instant), arcsec = RAD / 3600;
    const zeta = (2306.2181 * t + 0.30188 * t * t) * arcsec, z = (2306.2181 * t + 1.09468 * t * t) * arcsec, theta = (2004.3109 * t - 0.42665 * t * t) * arcsec;
    return product(horizon(place, instant), product(aboutZ(z), product(aboutY(theta), aboutZ(zeta))));
}

// The largest terms of tables 47.A and 47.B: multiples of D, M, M′ and F, then the sine terms of
// longitude (1e-6°) and the cosine terms of distance (m); the terms with M scale by E per power of M.
const LONGITUDE: number[][] = [
    [0, 0, 1, 0, 6288774, -20905355], [2, 0, -1, 0, 1274027, -3699111], [2, 0, 0, 0, 658314, -2955968], [0, 0, 2, 0, 213618, -569925],
    [0, 1, 0, 0, -185116, 48888], [0, 0, 0, 2, -114332, -3149], [2, 0, -2, 0, 58793, 246158], [2, -1, -1, 0, 57066, -152138],
    [2, 0, 1, 0, 53322, -170733], [2, -1, 0, 0, 45758, -204586], [0, 1, -1, 0, -40923, -129620], [1, 0, 0, 0, -34720, 108743],
    [0, 1, 1, 0, -30383, 104755], [2, 0, 0, -2, 15327, 10321], [0, 0, 1, 2, -12528, 0], [0, 0, 1, -2, 10980, 79661],
    [4, 0, -1, 0, 10675, -34782], [0, 0, 3, 0, 10034, -23210], [4, 0, -2, 0, 8548, -21636], [2, 1, -1, 0, -7888, 24208],
    [2, 1, 0, 0, -6766, 30824], [1, 0, -1, 0, -5163, -8379], [1, 1, 0, 0, 4987, -16675], [2, -1, 1, 0, 4036, -12831], [2, 0, 2, 0, 3994, -10445],
];
// The sine terms of latitude (1e-6°).
const LATITUDE: number[][] = [
    [0, 0, 0, 1, 5128122], [0, 0, 1, 1, 280602], [0, 0, 1, -1, 277693], [2, 0, 0, -1, 173237], [2, 0, -1, 1, 55413], [2, 0, -1, -1, 46271],
    [2, 0, 0, 1, 32573], [0, 0, 2, 1, 17198], [2, 0, 1, -1, 9266], [0, 0, 2, -1, 8822], [2, -1, 0, -1, 8216], [2, 0, -2, -1, 4324], [2, 0, 1, 1, 4200],
];

/** The moon's geocentric ecliptic longitude and latitude of the date in degrees, and its distance in km. */
export function moonEcliptic(instant: number): { longitude: number; latitude: number; km: number } {
    const { t } = epoch(instant);
    const mean = 218.3164477 + 481267.88123421 * t, d = 297.8501921 + 445267.1114034 * t, m = 357.5291092 + 35999.0502909 * t;
    const m1 = 134.9633964 + 477198.8675055 * t, f = 93.272095 + 483202.0175233 * t, e = 1 - 0.002516 * t;
    const a1 = 119.75 + 131.849 * t, a2 = 53.09 + 479264.29 * t, a3 = 313.45 + 481266.484 * t;
    const angle = (k: number[]) => (k[0] * d + k[1] * m + k[2] * m1 + k[3] * f) * RAD, scale = (k: number[]) => e ** Math.abs(k[1]);
    let l = 3958 * Math.sin(a1 * RAD) + 1962 * Math.sin((mean - f) * RAD) + 318 * Math.sin(a2 * RAD), r = 0;
    for (const k of LONGITUDE) {
        l += k[4] * scale(k) * Math.sin(angle(k));
        r += k[5] * scale(k) * Math.cos(angle(k));
    }
    let b = -2235 * Math.sin(mean * RAD) + 382 * Math.sin(a3 * RAD) + 175 * Math.sin((a1 - f) * RAD) + 175 * Math.sin((a1 + f) * RAD)
        + 127 * Math.sin((mean - m1) * RAD) - 115 * Math.sin((mean + m1) * RAD);
    for (const k of LATITUDE) b += k[4] * scale(k) * Math.sin(angle(k));
    return { longitude: (((mean + l / 1e6) % 360) + 360) % 360, latitude: b / 1e6, km: 385000.56 + r / 1000 };
}

/** Toward the moon from a place on the ground, east, north and up: the topocentric direction. */
export function moonAt(place: [number, number], instant: number): Vec3 {
    const { longitude, latitude, km } = moonEcliptic(instant), { t } = epoch(instant);
    const l = longitude * RAD, b = latitude * RAD, obliquity = (23.439291 - 0.0130042 * t) * RAD;
    const geocentric: Vec3 = [
        km * Math.cos(b) * Math.cos(l),
        km * (Math.cos(obliquity) * Math.cos(b) * Math.sin(l) - Math.sin(obliquity) * Math.sin(b)),
        km * (Math.sin(obliquity) * Math.cos(b) * Math.sin(l) + Math.cos(obliquity) * Math.sin(b)),
    ];
    // Seen from the place, the moon lies lower by the place's offset from the Earth's centre: its up.
    const [e, n, u] = times(horizon(place, instant), geocentric), length = Math.hypot(e, n, u - EARTH_KM);
    return [e / length, n / length, (u - EARTH_KM) / length];
}
