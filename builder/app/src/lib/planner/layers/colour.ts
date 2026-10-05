import type { Swatch, Theme } from './data-layer';

/** The channels of a `#rrggbb` colour. */
const rgb = (hex: string): number[] => [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16));

/** An ImageData pixel as one word of a little-endian Uint32Array view. */
export function abgr(hex: string, alpha = 255): number {
    const [r, g, b] = rgb(hex);
    return ((alpha << 24) | (b << 16) | (g << 8) | r) >>> 0;
}

/** The colour a share `t` of the way from `a` to `b`. */
export function mix(a: string, b: string, t: number): string {
    const from = rgb(a), to = rgb(b);
    return '#' + from.map((c, i) => Math.round(c + t * (to[i] - c)).toString(16).padStart(2, '0')).join('');
}

/** The hatched swatch of missing data, the same in every layer. */
export function noData(theme: Theme): Swatch {
    return { label: 'No data', color: theme === 'dark' ? '#6b685c' : '#b8b5ac', hatch: true };
}
