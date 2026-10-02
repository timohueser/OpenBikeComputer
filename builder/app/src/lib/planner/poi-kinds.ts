/**
 * Rider place categories: `label` names one place, `plural` the layer, `icon` is a 24 px stroke path, `kinds` maps basemap
 * `pois` kinds to names. The places bake (`tools/planner_places.py`) and the iOS replay assets read the same JSON file.
 */
import placeCategories from './poi-kinds.json' with { type: 'json' };

export { placeCategories };
export type PlaceCategory = keyof typeof placeCategories;
export const categoryIds = Object.keys(placeCategories) as PlaceCategory[];

/** Basemap `pois` kinds the planner shows as places. */
export const poiKinds: Record<string, { category: PlaceCategory; label: string }> = Object.fromEntries(
    categoryIds.flatMap(category => Object.entries(placeCategories[category].kinds).map(([kind, label]) => [kind, { category, label }])),
);
