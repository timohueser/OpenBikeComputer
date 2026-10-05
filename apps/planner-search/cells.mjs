import {DatabaseSync} from 'node:sqlite';
import {pathToFileURL} from 'node:url';
import {federate} from './federation.mjs';

/** Opens search cell files read-only for the shared federation. A connection attaches at most
 * eight cells, below SQLite's default attachment limit; its own schema is private memory. */
export function openCells(files) {
  const groups = [];
  const close = () => { for (const group of groups) group.database.close(); };
  try {
    for (let start = 0; start < files.length; start += 8) {
      const group = {database: new DatabaseSync(':memory:'), names: [], statements: new Map()};
      groups.push(group);
      group.database.exec('PRAGMA temp_store=MEMORY');
      for (let i = start; i < Math.min(files.length, start + 8); i++) {
        group.database.prepare(`ATTACH DATABASE ? AS c${i}`).run(`${pathToFileURL(files[i]).href}?mode=ro`);
        group.database.exec(`PRAGMA c${i}.cache_size=-${Math.max(128, Math.floor(16384 / files.length))}; PRAGMA c${i}.mmap_size=0`);
        group.names.push(`c${i}`);
      }
    }
    const run = (index, sql, params) => {
      const {database, statements} = groups[index];
      if (!statements.has(sql)) {
        if (statements.size >= 128) statements.delete(statements.keys().next().value);
        statements.set(sql, database.prepare(sql));
      }
      return statements.get(sql).all(...params);
    };
    return {...federate({groups: () => groups.map(group => group.names), run}), close};
  } catch (error) { close(); throw error; }
}
