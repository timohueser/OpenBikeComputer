const overlaps = (a, b) => !a || !b || a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1];
const typeOrder = value => value === null || value === undefined ? 0 : typeof value === 'number' ? 1 : 2;
// SQLite order: NULL, numbers, text. Text keys are ASCII, so code-unit order equals BINARY order.
const compare = (a, b) => typeOrder(a) - typeOrder(b) || (a < b ? -1 : a > b ? 1 : 0);

/**
 * Search cells keep their own indexes. `groups()` lists the attached cell schemas of each
 * connection, and `run(group, sql, params)` returns the rows of one statement on it.
 *
 * A cell query names cell tables as `{c}.table` and may declare `order` (result columns,
 * a leading `-` sorts descending), `limit` and `bounds` (cells outside are skipped). Each
 * cell applies the order and limit; the merge applies them again, so the limit is global.
 * Result columns that start with `_` only order the merge. Rows with an `id` are one place.
 */
export function federate({groups, run}) {
  const layout = groups().map((names, group) => names.map(name => ({name, group})));
  const cells = layout.flat();
  if (!cells.length) throw new Error('No search cells are installed.');
  for (const cell of cells)
    cell.metadata = Object.fromEntries(run(cell.group, `SELECT key,value FROM ${cell.name}.metadata`, [])
      .map(row => [row.key, JSON.parse(row.value)]));
  const first = cells[0].metadata;
  if (cells.some(({metadata}) => metadata.schema !== 5 || metadata.osm_sha256 !== first.osm_sha256))
    throw new Error('Search cells need schema 5 and one OSM source.');
  // The region metadata is what every cell shares.
  const metadata = Object.fromEntries(Object.entries(first).filter(([key, value]) =>
    cells.every(cell => JSON.stringify(cell.metadata[key]) === JSON.stringify(value))));

  // One region lexicon keeps spelling corrections and their ranks independent of the cells.
  run(0, 'CREATE TABLE main.lexicon(term TEXT UNIQUE)', []);
  run(0, "CREATE VIRTUAL TABLE main.fuzzy USING fts5(term,content='lexicon',detail=none,tokenize='trigram')", []);
  for (const cell of cells) {
    const terms = run(cell.group, `SELECT term FROM ${cell.name}.lexicon`, []).map(row => row.term);
    run(0, 'INSERT OR IGNORE INTO main.lexicon(term) SELECT value FROM json_each(?)', [JSON.stringify(terms)]);
  }
  run(0, "INSERT INTO main.fuzzy(fuzzy) VALUES('rebuild')", []);
  run(0, "INSERT INTO main.fuzzy(fuzzy) VALUES('optimize')", []);

  function rows({sql, params = [], order = [], limit, bounds}) {
    if (!sql.includes('{c}.')) throw new Error('A cell query names its tables as {c}.table.');
    const keys = order.map(key => key[0] === '-' ? [key.slice(1), -1] : [key, 1]);
    const tail = (keys.length ? ' ORDER BY ' + keys.map(([key, sign]) => `"${key}"${sign < 0 ? ' DESC' : ''}`).join(',') : '')
      + (limit ? ` LIMIT ${limit}` : '');
    const result = [], seen = new Set();
    const take = found => {
      for (const row of found) {
        const names = Object.keys(row), visible = names.filter(name => name[0] !== '_');
        const value = visible.length === names.length ? row : Object.fromEntries(visible.map(name => [name, row[name]]));
        const identity = 'id' in value ? value.id : JSON.stringify(value);
        if (seen.has(identity)) continue;
        seen.add(identity); result.push(value);
        if (result.length === limit) return true;
      }
      return false;
    };
    let found = [];
    for (const members of layout) {
      const selected = members.filter(cell => overlaps(cell.metadata.bounds, bounds));
      if (!selected.length) continue;
      const batch = run(selected[0].group, selected.map(cell =>
        `SELECT * FROM (SELECT * FROM (${sql.replaceAll('{c}.', cell.name + '.')})${tail})`).join(' UNION ALL '),
      selected.flatMap(() => params));
      if (keys.length) found = found.concat(batch);
      else if (take(batch)) return result;
    }
    found.sort((a, b) => {
      for (const [key, sign] of keys) { const c = compare(a[key], b[key]); if (c) return c * sign; }
      return 0;
    });
    take(found);
    return result;
  }

  /** Records of the places that the queries select by `id`, in the order of first selection. */
  function places(queries) {
    const ids = [...new Set(queries.flatMap(query => rows(query).map(row => row.id)))];
    if (!ids.length) return [];
    const records = new Map(rows({sql: 'SELECT p.* FROM {c}.places p WHERE p.id IN (SELECT value FROM json_each(?))',
      params: [JSON.stringify(ids)]}).map(record => [record.id, record]));
    return ids.flatMap(id => records.get(id) ?? []);
  }

  return {metadata, rows, places, lexicon: (sql, params = []) => run(0, sql, params)};
}

/** A native adapter exchanges JSON text: `run` replies `{"rows":[...]}` or `{"error":"..."}`. */
export function nativeCells({groups, run}) {
  return federate({groups: () => JSON.parse(groups()), run: (group, sql, params) => {
    const reply = JSON.parse(run(group, sql, JSON.stringify(params)));
    if (reply.error) throw new Error(reply.error);
    return reply.rows;
  }});
}
