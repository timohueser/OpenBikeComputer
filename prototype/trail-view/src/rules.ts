/**
 * The rules of every region in a bake that paint the ground, the water and the roads, packed for
 * the terrain shader: one row of texels per region (the bake's region layer indexes it), one column
 * per slot. The shader reads each texel's own region, so a rule paints where it applies.
 */

import * as THREE from 'three';
import { COVER_ORDER, LITHOLOGIES, MATERIALS } from './materials';
import { DECIDUOUS, ramps, ROAD_LINE, ROADS } from './palette';
import type { Region } from './regions';
import { SEASONS } from './models/trees';

/**
 * The slots: three ramp colours per cover; a paint per material, alpha 1 where the region names
 * one; a colour per lithology, alpha 1 where it is banded; then one texel each for the cliff
 * colour (alpha 1 where the region names one), the heather (alpha its share), the speckle (alpha
 * its share), the terrain scalars (outcrops, cos turf_deg, snowline, speckle size), the bedding
 * plane's normal (alpha the season's bare_luma, 0 for none) and the palette snap; then the sea's
 * deep and shallow colours, the road surfaces in ROADS' order and the edge line of main roads.
 */
export const SLOT = (() => {
  const materials = 3 * COVER_ORDER.length;
  const lithologies = materials + MATERIALS.length;
  const cliff = lithologies + LITHOLOGIES.length;
  const roads = cliff + 8;
  return {
    materials, lithologies, cliff, heather: cliff + 1, speckle: cliff + 2, terrain: cliff + 3, bedding: cliff + 4, snap: cliff + 5,
    sea: cliff + 6, roads, roadLine: roads + ROADS.length, count: roads + ROADS.length + 1,
  };
})();

/** The rows for a season (0 winter to 3 autumn), linear RGB. */
export function packRules(regions: Region[], season: number): Float32Array {
  const data = new Float32Array(regions.length * SLOT.count * 4);
  const colour = (hex: string) => new THREE.Color(hex);
  regions.forEach(({ leaf, look }, row) => {
    const set = (slot: number, rgb: THREE.Color | [number, number, number], a: number) =>
      data.set([...(rgb instanceof THREE.Color ? rgb.toArray() : rgb), a], (row * SLOT.count + slot) * 4);
    ramps(season, DECIDUOUS[leaf], look).forEach((c, i) => set(i, c, 1));
    MATERIALS.forEach((id, i) => {
      const yearly = look.materials?.[id]?.colour;
      const paint = typeof yearly === 'string' ? yearly : yearly?.[SEASONS[season]];
      set(SLOT.materials + i, colour(paint ?? '#000'), paint ? 1 : 0);
    });
    LITHOLOGIES.forEach((id, i) => {
      const rock = look.lithology?.[id];
      set(SLOT.lithologies + i, colour(rock?.colour ?? look.rock), rock?.banded ? 1 : 0);
    });
    set(SLOT.cliff, colour(look.cliff ?? '#000'), look.cliff ? 1 : 0);
    set(SLOT.heather, colour(look.moor?.heather ?? '#000'), look.moor?.share ?? 0);
    set(SLOT.speckle, colour(look.speckle?.colour ?? '#000'), look.speckle?.share ?? 0);
    // Out of winter, snow lies only above the snowline; in winter it lies wherever the satellite sees it.
    const snowline = SEASONS[season] === 'winter' ? -1e4 : look.snowline;
    set(SLOT.terrain, [look.outcrops, Math.cos((look.turf_deg * Math.PI) / 180), snowline], look.speckle?.size_m ?? 1);
    // The bedding plane's normal: up, tipped by the dip toward the dip direction, 90° right of the strike.
    const { strike, dip } = look.bedding ?? { strike: 0, dip: 0 };
    const [toward, tilt] = [((strike + 90) * Math.PI) / 180, (dip * Math.PI) / 180];
    const bare = Array.isArray(look.bare_luma) ? look.bare_luma[season] : look.bare_luma;
    set(SLOT.bedding, [Math.sin(toward) * Math.sin(tilt), Math.cos(toward) * Math.sin(tilt), Math.cos(tilt)], bare ?? 0);
    set(SLOT.snap, [look.snap, 0, 0], 0);
    set(SLOT.sea, colour(look.sea.deep), 1);
    set(SLOT.sea + 1, colour(look.sea.shallow), 1);
    (look.roads ?? ROADS).forEach((hex, i) => set(SLOT.roads + i, colour(hex), 1));
    set(SLOT.roadLine, colour(look.roadLine ?? ROAD_LINE), 1);
  });
  return data;
}
