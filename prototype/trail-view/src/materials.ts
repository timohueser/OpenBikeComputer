/**
 * The ground vocabulary of materials.json, which bake.py writes in codes: a material or a
 * lithology's code is its index. A material paints with its cover's ramp and carries its cover's
 * plants; a lithology gives the rock its hardness.
 */

import vocabulary from '../materials.json';

/** WorldCover's groups in the order of palette.ts's ramps. */
export const COVER_ORDER = ['tree', 'shrub', 'grass', 'crop', 'built', 'bare', 'snow', 'water', 'wetland', 'mangrove', 'moss'] as const;
export type CoverName = (typeof COVER_ORDER)[number];

export const MATERIALS = vocabulary.materials.map((m) => m.id);
export const LITHOLOGIES = vocabulary.lithologies.map((l) => l.id);
/** Each material code's cover, as an index into COVER_ORDER. */
export const COVER = vocabulary.materials.map((m) => COVER_ORDER.indexOf(m.cover as CoverName));
export const HARDNESS = vocabulary.lithologies.map((l) => l.hardness);

const code = (id: string) => MATERIALS.indexOf(id);
/** Heath and bog: where the heather grows and the moor plants stand. */
export const MOOR = new Set(['heath', 'bog'].map(code));
/** Exposed rock: crag and turf ledges together. It shows as slabs, stones and bare ground as far as the region's outcrops and the rock's hardness say. */
export const ROCKY = new Set(['bare-rock', 'scree'].map(code));
/** Open, unfarmed ground: slabs break through it and bushes stand on it. Pasture is cleared. */
export const ROUGH = new Set(
  MATERIALS.flatMap((id, i) => (['shrub', 'grass', 'bare', 'wetland', 'moss'].includes(COVER_ORDER[COVER[i]]) && id !== 'pasture' ? [i] : [])),
);
/** Per material code, whether it is water: the sea or inland water. */
export const WATERY = MATERIALS.map((id) => id === 'sea' || id === 'inland-water');

/** The vocabulary's own problems: unknown covers, and ids that repeat. */
export function vocabularyProblems(): string[] {
  const out: string[] = [];
  vocabulary.materials.forEach((m, i) => {
    if (COVER[i] < 0) out.push(`materials.json: materials[${i}].cover: ${m.cover} is not one of ${COVER_ORDER.join(', ')}`);
  });
  for (const [list, ids] of [['materials', MATERIALS], ['lithologies', LITHOLOGIES]] as const) {
    ids.forEach((id, i) => ids.indexOf(id) !== i && out.push(`materials.json: ${list}[${i}].id: ${id} is also ${list}[${ids.indexOf(id)}]`));
  }
  return out;
}
