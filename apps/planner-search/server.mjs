import http from 'node:http';
import { DatabaseSync } from 'node:sqlite';
import { existsSync, statSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { parserProcess } from './parser.mjs';
import { answerQuery } from './query.mjs';
import { routeQuery } from './routing.mjs';
import { validateInput } from './validation.mjs';

const root = import.meta.dirname,
  data = path.resolve(process.env.OBC_SEARCH_DATA || path.join(root, 'data'));
const parser = parserProcess(
  process.env.OBC_SEARCH_PYTHON || path.join(root, '.venv/bin/python'),
  path.join(data, 'model'),
);
const databases = new Map();
for (const region of (process.env.OBC_SEARCH_REGIONS || 'germany,baden-wuerttemberg').split(',')) {
  if (!['germany', 'baden-wuerttemberg'].includes(region)) throw new Error(`Unknown search region: ${region}`);
  const file = path.join(data, `${region}.sqlite`);
  if (!existsSync(file)) continue;
  const conn = new DatabaseSync(file, { readOnly: true });
  conn.exec(
    'PRAGMA query_only=ON; PRAGMA cache_size=-32768; PRAGMA mmap_size=0',
  );
  const statements = new Map();
  const db = {
    all(sql, params = []) {
      if (!statements.has(sql)) {
        if (statements.size >= 100) statements.clear();
        statements.set(sql, conn.prepare(sql));
      }
      return statements.get(sql).all(...params);
    },
  };
  const metadata = Object.fromEntries(
    db.all('SELECT * FROM metadata').map((r) => [r.key, JSON.parse(r.value)]),
  );
  if (metadata.schema !== 1)
    throw new Error(`Rebuild ${region}: incompatible search data.`);
  databases.set(region, {
    db,
    metadata,
    bytes: statSync(file).size,
    close: () => conn.close(),
  });
}
const server = http.createServer(async (req, res) => {
  const json = (status, value) => {
    res.writeHead(status, {
      'Content-Type': 'application/json',
      'Cache-Control': 'no-store',
    });
    res.end(JSON.stringify(value));
  };
  try {
    if (
      req.headers.origin &&
      !['localhost', '127.0.0.1', '[::1]'].includes(
        new URL(req.headers.origin).hostname,
      )
    ) {
      json(403, { error: 'Local access only.' });
      return;
    }
    const url = new URL(req.url, 'http://localhost');
    if (url.pathname === '/api/planner-search/status' && req.method === 'GET') {
      json(200, {
        parser: parser.status(),
        regions: [...databases].map(([id, v]) => ({
          id,
          metadata: v.metadata,
          bytes: v.bytes,
        })),
      });
      return;
    }
    if (url.pathname === '/api/planner-search/sample' && req.method === 'GET') {
      const xml = readFileSync(
        path.resolve(
          root,
          '../../fixtures/sources/route-import/komoot-schwarzwald.gpx',
        ),
        'utf8',
      );
      const coordinates = [
        ...xml.matchAll(/<trkpt lat="([^"]+)" lon="([^"]+)"/g),
      ].map((m) => [Number(m[2]), Number(m[1])]);
      json(200, { coordinates });
      return;
    }
    if (
      !['/api/planner-search/query', '/api/planner-search/route'].includes(
        url.pathname,
      ) ||
      req.method !== 'POST'
    ) {
      json(404, { error: 'Not found.' });
      return;
    }
    req.setEncoding('utf8');
    let body = '';
    for await (const chunk of req) {
      body += chunk;
      if (body.length > 2000000) {
        json(413, {
          error: 'This route is too large for the local query service.',
        });
        return;
      }
    }
    const input = JSON.parse(body);
    if (url.pathname === '/api/planner-search/route') {
      json(200, await routeQuery(input));
      return;
    }
    validateInput(input);
    const region = input.region || 'baden-wuerttemberg',
      database = databases.get(region);
    if (!database) {
      json(503, {
        error: `Build the ${region} search package first. See apps/planner-search/README.md.`,
      });
      return;
    }
    const { db } = database;
    const answer = await answerQuery(db, input, parser);
    json(200, {
      ...answer,
      region,
      attribution: database.metadata.attribution,
    });
  } catch (error) {
    json(400, { error: error.message });
  }
});
const port = Number(process.env.OBC_SEARCH_PORT || 8780);
server.listen(port, '127.0.0.1', () =>
  console.log(`Local planner search: http://127.0.0.1:${port}`),
);
let stopping = false;
const stop = () => {
  if (stopping) return;
  stopping = true;
  parser.close();
  server.close(() => {
    for (const v of databases.values()) v.close();
  });
};
process.on('SIGINT', stop);
process.on('SIGTERM', stop);
