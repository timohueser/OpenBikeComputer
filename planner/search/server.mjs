import http from 'node:http';
import path from 'node:path';
import { parserProcess } from './parser.mjs';
import { searchRuntime } from './runtime.mjs';
import {openRegion} from './installation.mjs';
import { openingHours } from './hours.mjs';
import { RequestError } from './validation.mjs';
import { allowedOrigin } from './origins.mjs';

const root = import.meta.dirname,
  data = path.resolve(process.env.OBC_SEARCH_DATA || path.join(root, 'data'));
// One process serves one region; the deployment runs one process per region.
const region = process.env.OBC_SEARCH_REGIONS || 'baden-wuerttemberg';
if (!/^[a-z][a-z0-9-]{0,63}$/.test(region)) throw new Error(`Invalid search region: ${region}`);
const installed = openRegion(data, region);
if (!installed) throw new Error(`Missing search data for ${region} in ${data}.`);
const { db, bytes } = installed;
const parser = parserProcess(
  process.env.OBC_SEARCH_PYTHON || path.join(root, '.venv/bin/python'),
  path.join(data, 'model'),
  (message) => {
    // The supervisor restarts the service, and with it the model.
    console.error(message);
    process.exitCode = 1;
    stop();
    setTimeout(() => process.exit(1), 1000).unref();
  },
);
const runtime = searchRuntime({ db, parser, hours: openingHours(db.metadata.time_zone) });
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
        regions: [{ id: region, grid: installed.grid, metadata: db.metadata, bytes }],
      });
      return;
    }
    if (
      !['/api/planner-search/query', '/api/planner-search/reverse'].includes(url.pathname) ||
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
    let input;
    try {
      input = JSON.parse(body);
    } catch {
      throw new RequestError('The request body is not JSON.');
    }
    if (url.pathname === '/api/planner-search/reverse') {
      json(200, runtime.reverse(input?.coordinate));
      return;
    }
    json(200, await runtime.query(input));
  } catch (error) {
    if (error instanceof RequestError) json(400, { error: error.message });
    else {
      console.error(error);
      json(500, { error: 'Search failed. Try again.' });
    }
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
  server.close(() => db.close());
};
process.on('SIGINT', stop);
process.on('SIGTERM', stop);
