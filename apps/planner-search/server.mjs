import http from 'node:http';
import { DatabaseSync } from 'node:sqlite';
import { existsSync, statSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { parserProcess } from './parser.mjs';
import { answerQuery } from './query.mjs';
import { routeQuery } from './routing.mjs';
import { validateInput } from './validation.mjs';
import { allowedOrigin } from './origins.mjs';
import { reverseAddress } from './web/reverse.mjs';

const root = import.meta.dirname,
  data = path.resolve(process.env.OBC_SEARCH_DATA || path.join(root, 'data'));
const parser = parserProcess(
  process.env.OBC_SEARCH_PYTHON || path.join(root, '.venv/bin/python'),
  path.join(data, 'model'),
);
const databases = new Map();
for (const region of (process.env.OBC_SEARCH_REGIONS || 'germany,baden-wuerttemberg').split(',')) {
  if (!/^[a-z][a-z0-9-]{0,63}$/.test(region)) throw new Error(`Invalid search region: ${region}`);
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
  if (metadata.schema !== 2)
    throw new Error(`Rebuild ${region}: incompatible search data.`);
  conn.prepare('SELECT id FROM address_spatial LIMIT 0');
  databases.set(region, {
    db,
    metadata,
    bytes: statSync(file).size,
    close: () => conn.close(),
  });
}
const origins = new Set((process.env.OBC_SEARCH_ORIGINS || '').split(',').filter(Boolean));
let active = 0;
const server = http.createServer(async (req, res) => {
  const origin = req.headers.origin;
  const allowed = allowedOrigin(origin, origins);
  const cors = allowed && origin ? { 'Access-Control-Allow-Origin': origin, Vary: 'Origin' } : {};
  const json = (status, value) => {
    res.writeHead(status, {
      'Content-Type': 'application/json',
      'Cache-Control': 'no-store',
      ...cors,
    });
    res.end(JSON.stringify(value));
  };
  let admitted = false;
  try {
    if (!allowed) {
      json(403, { error: 'Origin is not allowed.' });
      return;
    }
    if (req.method === 'OPTIONS') {
      res.writeHead(204, { ...cors, 'Access-Control-Allow-Methods': 'GET, POST, OPTIONS', 'Access-Control-Allow-Headers': 'Content-Type', 'Access-Control-Max-Age': '600' });
      res.end();
      return;
    }
    if (active >= 16) { json(503, { error: 'Search is busy. Retry shortly.' }); return; }
    active++;
    admitted = true;
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
        process.env.OBC_SEARCH_SAMPLE || path.resolve(
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
      !['/api/planner-search/query', '/api/planner-search/route', '/api/planner-search/reverse'].includes(
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
          error: 'This route is too large for search.',
        });
        return;
      }
    }
    const input = JSON.parse(body);
    if (url.pathname === '/api/planner-search/route') {
      json(200, await routeQuery(input));
      return;
    }
    if (url.pathname !== '/api/planner-search/reverse') validateInput(input);
    const region = input.region || 'baden-wuerttemberg',
      database = databases.get(region);
    if (!database) {
      json(503, {
        error: 'Search does not cover this region.',
      });
      return;
    }
    const { db } = database;
    if (url.pathname === '/api/planner-search/reverse') {
      json(200, {label: reverseAddress(db, input.coordinate)});
      return;
    }
    const answer = await answerQuery(db, input, parser);
    json(200, {
      ...answer,
      region,
      attribution: database.metadata.attribution,
    });
  } catch (error) {
    json(400, { error: error.message });
  } finally {
    if (admitted) active--;
  }
});
const port = Number(process.env.OBC_SEARCH_PORT || 8780);
server.requestTimeout = 15000;
server.headersTimeout = 10000;
server.maxConnections = 64;
server.setTimeout(30000);
server.listen(port, '127.0.0.1', () =>
  console.log(`Local planner search: http://127.0.0.1:${server.address().port}`),
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
