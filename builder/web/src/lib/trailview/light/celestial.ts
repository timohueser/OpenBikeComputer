/**
 * The sky about a place at an instant: the rotation from the equator of the stars (J2000) to east,
 * north and up, and the sun and the moon there. The moon is the series of Meeus, Astronomical
 * Algorithms, chapter 47, in full: about 10″ off. The sun is that of chapter 25: a few ″ off. Both
 * stand where a place on the ellipsoid sees them, so the moon covers the sun where an eclipse is:
 * 1″ between them moves the moon's shadow by about 2 km.
 */

/** A rotation, rows first. */
export type Mat3 = [number, number, number, number, number, number, number, number, number];
type Vec3 = [number, number, number];

/** A body as a place sees it: toward its centre in east, north and up, and its radius in radians. */
export interface Body {
    toward: Vec3;
    radius: number;
}

const RAD = Math.PI / 180;
/** The Earth's equatorial radius in km, and the square of the ellipsoid's eccentricity (WGS 84). */
const EARTH_KM = 6378.137, EARTH_E2 = 0.00669438;
/** The radius of the moon and of the sun in km, and the astronomical unit. */
const MOON_KM = 1737.4, SUN_KM = 696000, AU_KM = 149597870.7;
/** TT − UTC since 2017 in ms: the sun and the moon move in TT, the Earth turns in UT. */
const TT_UTC = 69184;

const times = (m: Mat3, [x, y, z]: Vec3): Vec3 => [m[0] * x + m[1] * y + m[2] * z, m[3] * x + m[4] * y + m[5] * z, m[6] * x + m[7] * y + m[8] * z];
const product = (a: Mat3, b: Mat3): Mat3 => [0, 1, 2].flatMap(i => [0, 1, 2].map(j => a[3 * i] * b[j] + a[3 * i + 1] * b[3 + j] + a[3 * i + 2] * b[6 + j])) as Mat3;
const aboutZ = (a: number): Mat3 => [Math.cos(a), -Math.sin(a), 0, Math.sin(a), Math.cos(a), 0, 0, 0, 1];
const aboutY = (a: number): Mat3 => [Math.cos(a), 0, -Math.sin(a), 0, 1, 0, Math.sin(a), 0, Math.cos(a)];

/** Days and Julian centuries from J2000.0. */
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

// Tables 47.A and 47.B: multiples of D, M, M′ and F, then the sine terms of longitude (1e-6°) and the
// cosine terms of distance (m), or the sine terms of latitude; the terms with M scale by E per power of M.
const LONGITUDE: number[][] = [
    [0, 0, 1, 0, 6288774, -20905355], [2, 0, -1, 0, 1274027, -3699111], [2, 0, 0, 0, 658314, -2955968], [0, 0, 2, 0, 213618, -569925],
    [0, 1, 0, 0, -185116, 48888], [0, 0, 0, 2, -114332, -3149], [2, 0, -2, 0, 58793, 246158], [2, -1, -1, 0, 57066, -152138],
    [2, 0, 1, 0, 53322, -170733], [2, -1, 0, 0, 45758, -204586], [0, 1, -1, 0, -40923, -129620], [1, 0, 0, 0, -34720, 108743],
    [0, 1, 1, 0, -30383, 104755], [2, 0, 0, -2, 15327, 10321], [0, 0, 1, 2, -12528, 0], [0, 0, 1, -2, 10980, 79661],
    [4, 0, -1, 0, 10675, -34782], [0, 0, 3, 0, 10034, -23210], [4, 0, -2, 0, 8548, -21636], [2, 1, -1, 0, -7888, 24208],
    [2, 1, 0, 0, -6766, 30824], [1, 0, -1, 0, -5163, -8379], [1, 1, 0, 0, 4987, -16675], [2, -1, 1, 0, 4036, -12831],
    [2, 0, 2, 0, 3994, -10445], [4, 0, 0, 0, 3861, -11650], [2, 0, -3, 0, 3665, 14403], [0, 1, -2, 0, -2689, -7003],
    [2, 0, -1, 2, -2602, 0], [2, -1, -2, 0, 2390, 10056], [1, 0, 1, 0, -2348, 6322], [2, -2, 0, 0, 2236, -9884],
    [0, 1, 2, 0, -2120, 5751], [0, 2, 0, 0, -2069, 0], [2, -2, -1, 0, 2048, -4950], [2, 0, 1, -2, -1773, 4130],
    [2, 0, 0, 2, -1595, 0], [4, -1, -1, 0, 1215, -3958], [0, 0, 2, 2, -1110, 0], [3, 0, -1, 0, -892, 3258],
    [2, 1, 1, 0, -810, 2616], [4, -1, -2, 0, 759, -1897], [0, 2, -1, 0, -713, -2117], [2, 2, -1, 0, -700, 2354],
    [2, 1, -2, 0, 691, 0], [2, -1, 0, -2, 596, 0], [4, 0, 1, 0, 549, -1423], [0, 0, 4, 0, 537, -1117],
    [4, -1, 0, 0, 520, -1571], [1, 0, -2, 0, -487, -1739], [2, 1, 0, -2, -399, 0], [0, 0, 2, -2, -381, -4421],
    [1, 1, 1, 0, 351, 0], [3, 0, -2, 0, -340, 0], [4, 0, -3, 0, 330, 0], [2, -1, 2, 0, 327, 0],
    [0, 2, 1, 0, -323, 1165], [1, 1, -1, 0, 299, 0], [2, 0, 3, 0, 294, 0], [2, 0, -1, -2, 0, 8752],
];
const LATITUDE: number[][] = [
    [0, 0, 0, 1, 5128122], [0, 0, 1, 1, 280602], [0, 0, 1, -1, 277693], [2, 0, 0, -1, 173237], [2, 0, -1, 1, 55413], [2, 0, -1, -1, 46271],
    [2, 0, 0, 1, 32573], [0, 0, 2, 1, 17198], [2, 0, 1, -1, 9266], [0, 0, 2, -1, 8822], [2, -1, 0, -1, 8216], [2, 0, -2, -1, 4324],
    [2, 0, 1, 1, 4200], [2, 1, 0, -1, -3359], [2, -1, -1, 1, 2463], [2, -1, 0, 1, 2211], [2, -1, -1, -1, 2065], [0, 1, -1, -1, -1870],
    [4, 0, -1, -1, 1828], [0, 1, 0, 1, -1794], [0, 0, 0, 3, -1749], [0, 1, -1, 1, -1565], [1, 0, 0, 1, -1491], [0, 1, 1, 1, -1475],
    [0, 1, 1, -1, -1410], [0, 1, 0, -1, -1344], [1, 0, 0, -1, -1335], [0, 0, 3, 1, 1107], [4, 0, 0, -1, 1021], [4, 0, -1, 1, 833],
    [0, 0, 1, -3, 777], [4, 0, -2, 1, 671], [2, 0, 0, -3, 607], [2, 0, 2, -1, 596], [2, -1, 1, -1, 491], [2, 0, -2, 1, -451],
    [0, 0, 3, -1, 439], [2, 0, 2, 1, 422], [2, 0, -3, -1, 421], [2, 1, -1, 1, -366], [2, 1, 0, 1, -351], [4, 0, 0, 1, 331],
    [2, -1, 1, 1, 315], [2, -2, 0, -1, 302], [0, 0, 1, 3, -283], [2, 1, 1, -1, -229], [1, 1, 0, -1, 223], [1, 1, 0, 1, 223],
    [0, 1, -2, -1, -220], [2, 1, -1, -1, -220], [1, 0, 1, 1, -185], [2, -1, -2, -1, 181], [0, 1, 2, 1, -177], [4, 0, -2, -1, 176],
    [4, -1, -1, -1, 166], [1, 0, 1, -1, -164], [4, 0, 1, -1, 132], [1, 0, -1, -1, -119], [4, -1, 0, -1, 115], [2, -2, 0, 1, 107],
];

/** The moon's geocentric ecliptic longitude and latitude of the date in degrees, and its distance in km. */
export function moonEcliptic(instant: number): { longitude: number; latitude: number; km: number } {
    const { t } = epoch(instant + TT_UTC);
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

/**
 * The sun's geocentric ecliptic longitude of the date in degrees, with the aberration that shifts it
 * 20″ back, and its distance in km. Both bodies leave out the nutation, which moves them alike.
 */
export function sunEcliptic(instant: number): { longitude: number; km: number } {
    const { t } = epoch(instant + TT_UTC);
    const mean = 280.46646 + 36000.76983 * t + 0.0003032 * t * t, m = (357.52911 + 35999.05029 * t - 0.0001537 * t * t) * RAD;
    const e = 0.016708634 - 0.000042037 * t - 0.0000001267 * t * t;
    const c = (1.914602 - 0.004817 * t - 0.000014 * t * t) * Math.sin(m) + (0.019993 - 0.000101 * t) * Math.sin(2 * m) + 0.000289 * Math.sin(3 * m);
    const au = 1.000001018 * (1 - e * e) / (1 + e * Math.cos(m + c * RAD));
    return { longitude: (((mean + c - 0.00569 / au) % 360) + 360) % 360, km: au * AU_KM };
}

/** A body at an ecliptic longitude and latitude in degrees and a distance in km, as a place sees it. */
function seen(place: [number, number], instant: number, longitude: number, latitude: number, km: number, radius: number): Body {
    const { t } = epoch(instant + TT_UTC);
    const l = longitude * RAD, b = latitude * RAD, obliquity = (23.439291 - 0.0130042 * t) * RAD;
    const geocentric: Vec3 = [
        km * Math.cos(b) * Math.cos(l),
        km * (Math.cos(obliquity) * Math.cos(b) * Math.sin(l) - Math.sin(obliquity) * Math.sin(b)),
        km * (Math.sin(obliquity) * Math.cos(b) * Math.sin(l) + Math.cos(obliquity) * Math.sin(b)),
    ];
    // The place lies on the ellipsoid: up from the Earth's centre, and toward the equator by the flattening.
    const phi = place[1] * RAD, n = EARTH_KM / Math.sqrt(1 - EARTH_E2 * Math.sin(phi) ** 2);
    const [e, north, u] = times(horizon(place, instant), geocentric), at: Vec3 = [e, north + n * EARTH_E2 * Math.sin(phi) * Math.cos(phi), u - n * (1 - EARTH_E2 * Math.sin(phi) ** 2)];
    const length = Math.hypot(...at);
    return { toward: [at[0] / length, at[1] / length, at[2] / length], radius: Math.asin(radius / length) };
}

/** The moon from a place on the ground. */
export function moonAt(place: [number, number], instant: number): Body {
    const { longitude, latitude, km } = moonEcliptic(instant);
    return seen(place, instant, longitude, latitude, km, MOON_KM);
}

/** The sun from a place on the ground. */
export function sunAt(place: [number, number], instant: number): Body {
    const { longitude, km } = sunEcliptic(instant);
    return seen(place, instant, longitude, 0, km, SUN_KM);
}
