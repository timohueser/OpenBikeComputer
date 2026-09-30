export const SEARCH_URL = (import.meta.env.VITE_PLANNER_SEARCH_URL || '/api/planner-search').replace(/\/$/, '');
export const HOSTED_SEARCH = Boolean(import.meta.env.VITE_PLANNER_SEARCH_URL);
