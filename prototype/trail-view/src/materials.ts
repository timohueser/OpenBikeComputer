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
/** Exposed rock: crag and turf ledges together, strewn with stones, where nothing is planted. */
export const ROCKY = new Set(['bare-rock', 'scree'].map(code));
export const WATER = new Set(['sea', 'inland-water'].map(code));

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
