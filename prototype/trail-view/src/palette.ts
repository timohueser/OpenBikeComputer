/**
 * The painted ground colours: one ramp per WorldCover class, low NDVI to high, in the order of
 * `CLASSES` in bake.py. Grass, crops and trees change with the season; deciduous autumn colour
 * comes from this table, not from NDVI.
 */

import * as THREE from 'three';

type Ramp = [string, string, string];

const FIXED: Record<string, Ramp> = {
  shrub: ['#7a7344', '#6b7a40', '#5a7a3a'],
  // WorldCover's built-up ground in an Alpine village is mostly gardens between houses.
  built: ['#a39a8e', '#99987e', '#8a9568'],
  bare: ['#8f8a80', '#979084', '#8a8a72'],
  snow: ['#eef2f6', '#eef2f6', '#eef2f6'],
  water: ['#2c5874', '#2c5874', '#2c5874'],
  wetland: ['#7d8a5a', '#6f8a52', '#5f8a4c'],
  mangrove: ['#3e5a34', '#365a32', '#2f5530'],
  moss: ['#8d8d6a', '#86906a', '#7a8c60'],
};

type Seasonal = Record<'grass' | 'crop' | 'needle' | 'broad', Ramp>;

/** Winter, spring, summer, autumn: the order of SEASONS in light.ts and of the S2 quarters. */
const SEASONAL: Seasonal[] = [
  {
    grass: ['#8f8a66', '#87865f', '#7f8459'],
    crop: ['#9a8e6a', '#8e8a62', '#7f855a'],
    needle: ['#2f3d2d', '#2d3f2e', '#2b3f2e'],
    broad: ['#6e6252', '#6a6050', '#655d4d'],
  },
  {
    grass: ['#a3a65e', '#8ea657', '#7a9c4d'],
    crop: ['#a9a066', '#93a35a', '#7e9a50'],
    needle: ['#35502f', '#33532f', '#30532e'],
    broad: ['#7aa04e', '#6f9e48', '#62983f'],
  },
  {
    grass: ['#a8a05e', '#8a9c52', '#76924a'],
    crop: ['#b8a764', '#979e58', '#7d944e'],
    needle: ['#2f4a2c', '#2d4c2c', '#2a4b2b'],
    broad: ['#55783a', '#4f7a38', '#487834'],
  },
  {
    grass: ['#a39c64', '#929a58', '#82944f'],
    crop: ['#a8946a', '#9a9460', '#8a9058'],
    needle: ['#334530', '#31472f', '#2f472e'],
    broad: ['#a57a40', '#9a6f3a', '#8a6636'],
  },
];

/** Share of broadleaf trees for a preset's `default_leaf`. */
export const DECIDUOUS: Record<string, number> = { needleleaved: 0.2, mixed: 0.5, broadleaved: 1 };

export const ROCK = '#8c877d';
export const SHORE = '#4f8088';

/** The 11 class ramps for a season, as 33 colours. */
export function ramps(season: number, deciduous: number): THREE.Color[] {
  const table = SEASONAL[season];
  const tree = table.needle.map((needle, i) => new THREE.Color(needle).lerp(new THREE.Color(table.broad[i]), deciduous));
  const order: (Ramp | THREE.Color[])[] = [
    tree,
    FIXED.shrub,
    table.grass,
    table.crop,
    FIXED.built,
    FIXED.bare,
    FIXED.snow,
    FIXED.water,
    FIXED.wetland,
    FIXED.mangrove,
    FIXED.moss,
  ];
  return order.flatMap((ramp) => ramp.map((colour) => (colour instanceof THREE.Color ? colour : new THREE.Color(colour))));
}
