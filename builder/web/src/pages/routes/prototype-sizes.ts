/** Prototype for #3559: the sizes of the Routes page, from the address: `?row=0|1|2` and `?card=180|200|240`. */
const search = new URLSearchParams(location.search);
export const row = Number(search.get('row') ?? 0);
export const card = Number(search.get('card') ?? 0) || undefined;
/** Any variant brings the decisions of the owner: the wider list, the activity icon and "Open in planner". */
export const on = row > 0 || card !== undefined;
/** A row shows the map picture where a route has no cover, as a card does (`?rowpic=none`: as today, the line on a plain ground). */
export const rowPlace: 'card' | 'row' = search.get('rowpic') === 'none' ? 'row' : 'card';
