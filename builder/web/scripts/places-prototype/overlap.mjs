// node overlap.mjs [kinds]: for each area, zoom and mode, how many places the view holds, how many marks the map draws for them,
// how many marks touch another mark, and how many are half or more under another mark.
import { AREAS, launch, open, view } from './lib.mjs';
const kinds = process.argv[2] ?? 'default';
const browser = await launch();
const modes = [['today', ''], ['groups', ''], ['badges', ''], ['badges', '&mix']];
const rows = [];
for (const [area, center] of Object.entries(AREAS)) for (const [mode, extra] of modes) {
    const { page, context } = await open(browser, { mode, kinds, extra, scale: 1 });
    for (const zoom of area === 'town' ? [11, 12, 13, 14, 15, 16, 17] : [11, 12, 13, 14, 15, 16]) {
        await view(page, center, zoom);
        const row = await page.evaluate(() => {
            const proto = window.__places, { map } = proto, bounds = map.getBounds();
            if (proto.mode === 'today') {
                const inView = f => bounds.contains([Number(f.properties.lon), Number(f.properties.lat)]);
                const filter = map.getFilter('planner-poi-icons');
                const all = new Set(map.querySourceFeatures('places', { sourceLayer: 'pois', filter }).filter(inView).map(f => f.id));
                const shown = new Set(map.queryRenderedFeatures({ layers: ['planner-pois', 'planner-poi-icons'] }).filter(inView).map(f => f.id));
                return { places: map.getZoom() < 12 ? null : all.size, marks: shown.size, placesShown: shown.size };
            }
            const items = proto.drawn().filter(item => item.x >= 0 && item.x <= 900 && item.y >= 0 && item.y <= 650);
            const box = item => {
                const kinds = item.kinds.split('.').length, digits = String(item.count).length;
                const w = item.count === 1 ? 20 : kinds === 1 ? 28 + 6.5 * digits : 11 + 16 * Math.min(kinds, 4) + (kinds > 4 ? 17 : 0), h = kinds > 1 ? 22 : 20;
                return { x0: item.x - w / 2, x1: item.x + w / 2, y0: item.y - h / 2, y1: item.y + h / 2, area: w * h };
            };
            const boxes = items.map(box);
            let touching = 0, hidden = 0;
            boxes.forEach((a, i) => {
                let touch = false, most = 0;
                boxes.forEach((b, j) => {
                    if (i === j) return;
                    const w = Math.min(a.x1, b.x1) - Math.max(a.x0, b.x0), h = Math.min(a.y1, b.y1) - Math.max(a.y0, b.y0);
                    if (w > 0 && h > 0) { touch = true; if (j < i) most = Math.max(most, w * h / a.area); }
                });
                if (touch) touching++;
                if (most >= 0.5) hidden++;
            });
            return { places: items.reduce((sum, item) => sum + item.count, 0), marks: items.length, touching, halfHidden: hidden };
        });
        rows.push({ area, zoom, mode: mode + extra.replace('&', '+'), ...row });
        console.log(JSON.stringify(rows.at(-1)));
    }
    await context.close();
}
await browser.close();
