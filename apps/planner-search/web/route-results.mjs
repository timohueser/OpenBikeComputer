// Refine the largest remaining route interval so Show more keeps every earlier selection.
export function routeResults(results, range, limit) {
  const order = (a, b) => a.position.along - b.position.along || a.source.localeCompare(b.source);
  const intervals = [{ from: range[0], to: range[1], rows: [...results].sort(order) }];
  const selected = [];
  while (selected.length < limit && intervals.length) {
    intervals.sort((a, b) => (b.to - b.from) - (a.to - a.from) || a.from - b.from);
    const { from, to, rows } = intervals.shift();
    if (!rows.length) continue;
    const middle = (from + to) / 2;
    const score = p => Math.abs(p.position.along - middle) + p.position.distance;
    let best = 0;
    for (let i = 1; i < rows.length; i++) if (score(rows[i]) < score(rows[best])) best = i;
    const point = rows[best];
    selected.push(point);
    if (best) intervals.push({ from, to: point.position.along, rows: rows.slice(0, best) });
    if (best + 1 < rows.length) intervals.push({ from: point.position.along, to, rows: rows.slice(best + 1) });
  }
  return selected.sort(order);
}
