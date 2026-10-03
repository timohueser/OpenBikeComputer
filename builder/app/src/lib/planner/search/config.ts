import { clientConfig } from "../client-config";
export const SEARCH_URL = (import.meta.env.VITE_PLANNER_SEARCH_URL || '/api/planner-search').replace(/\/$/, '');
export const HOSTED_SEARCH = Boolean(import.meta.env.VITE_PLANNER_SEARCH_URL);
export const SEARCH_REGIONS = clientConfig.region ? [clientConfig.region] : (import.meta.env.VITE_PLANNER_SEARCH_REGIONS || "baden-wuerttemberg,germany").split(",");
const regionNames: Record<string, string> = { germany: 'Germany', 'baden-wuerttemberg': 'Baden-Württemberg' };
/** The display name of a search package. */
export const regionName = (id: string) => regionNames[id] ?? id.replaceAll('-', ' ');
/** The served region: the name in its recipe, else the name of its search package. */
export const REGION_NAME = import.meta.env.VITE_PLANNER_REGION_NAME || regionName(SEARCH_REGIONS[0]);
