export const SEARCH_URL = (import.meta.env.VITE_PLANNER_SEARCH_URL || '/api/planner-search').replace(/\/$/, '');
export const HOSTED_SEARCH = Boolean(import.meta.env.VITE_PLANNER_SEARCH_URL);
const SEARCH_REGION = import.meta.env.VITE_PLANNER_SEARCH_REGIONS || 'baden-wuerttemberg';
const regionNames: Record<string, string> = { germany: 'Germany', 'baden-wuerttemberg': 'Baden-Württemberg', 'baden-wuerttemberg-switzerland': 'Baden-Württemberg and Switzerland' };
/** The served region: the name in its recipe, else the name of its search package. */
export const REGION_NAME = import.meta.env.VITE_PLANNER_REGION_NAME || regionNames[SEARCH_REGION] || SEARCH_REGION.replaceAll('-', ' ');
