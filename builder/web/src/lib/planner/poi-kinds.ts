/** Rider-place membership comes from search; labels and 24 px stroke icons belong to presentation. */
import membership from '../../../../../planner/search/place-kinds.json' with { type: 'json' };
import presentation from './poi-kinds.json' with { type: 'json' };

export type PlaceCategory = keyof typeof membership;
export const categoryIds = Object.keys(membership) as PlaceCategory[];
export const placeCategories = Object.fromEntries(categoryIds.map(category => [category, {
    ...presentation.categories[category],
    kinds: Object.fromEntries(membership[category].map(kind => [kind, presentation.labels[kind as keyof typeof presentation.labels]])),
}])) as Record<PlaceCategory, { label: string; plural: string; icon: string; kinds: Record<string, string> }>;

/** Search POI kinds the planner shows as places. */
export const poiKinds: Record<string, { category: PlaceCategory; label: string }> = Object.fromEntries(
    categoryIds.flatMap(category => Object.entries(placeCategories[category].kinds).map(([kind, label]) => [kind, { category, label }])),
);
