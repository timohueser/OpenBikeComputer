/**
 * The census: every pack entry with the model it draws and the regions that draw it. An entry whose
 * `latin` is not its model's stands in for a species the library has no model of yet.
 */

import { PACK_NAMES, packNamed, regionNamed, REGION_NAMES } from '../regions';
import { ANIMALS, PLANTS, TREES } from './library';

export interface Use {
  pack: string;
  /** The entry's key in the pack. */
  id: string;
  model: string;
  /** The species a stand-in stands in for; absent where the entry is its model's own species. */
  latin?: string;
  /** The regions whose pack this is. */
  regions: string[];
}

const LATIN: Record<string, string> = Object.fromEntries(Object.entries({ ...TREES, ...PLANTS, ...ANIMALS }).map(([id, m]) => [id, m.latin]));
const PACK_OF = Object.fromEntries(REGION_NAMES.map((id) => [id, regionNamed(id).pack.id]));

/** Base first, then each pack: its trees, ground plants, herds and orchard. */
export const USES: Use[] = ['base', ...PACK_NAMES].flatMap((name) => {
  const pack = packNamed(name);
  const regions = REGION_NAMES.filter((id) => PACK_OF[id] === name);
  type Entry = [{ id: string; latin?: string }, string];
  const entries: Entry[] = [
    ...pack.trees.map((s): Entry => [s, s.tree]),
    ...pack.ground.map((g): Entry => [g, g.plant]),
    ...pack.herds.map((h): Entry => [h, h.animal]),
    ...(pack.orchard ? [[{ id: 'orchard' }, pack.orchard] satisfies Entry] : []),
  ];
  return entries.map(([{ id, latin }, model]) => ({ pack: name, id, model, latin: latin !== LATIN[model] ? latin : undefined, regions }));
});

/** The regions of these uses, each once. */
export const regionsOf = (uses: Use[]) => [...new Set(uses.flatMap((use) => use.regions))];

/** The stand-ins among these uses, by the species they stand in for, in order of Latin name. */
export function standIns(uses: Use[]): [string, Use[]][] {
  const out = new Map<string, Use[]>();
  for (const use of uses) if (use.latin) out.set(use.latin, [...(out.get(use.latin) ?? []), use]);
  return [...out].sort(([a], [b]) => (a < b ? -1 : 1));
}
