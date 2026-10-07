import http from 'node:http';
import {open, realpath} from 'node:fs/promises';
import {Readable} from 'node:stream';
import {pipeline} from 'node:stream/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {fetch as tiles} from './worker.mjs';

/** Only the immutable prepared view is visible to the existing tile handler. */
export class Files {
  constructor(root) { this.root = root; }
  async get(key, options = {}) {
    if (!/^((?:planner|cell-catalog)\/objects\/[a-f0-9]{64}|cell-catalog\/releases\/[a-f0-9]{64}\/catalog\.json|planner\/releases\/[a-f0-9]{64}\/public\/[A-Za-z0-9 _.,@/-]+)$/.test(key)
        || key.split('/').includes('..')) return null;
    let file;
    try {
      const name = await realpath(path.join(this.root, key));
      if (!name.startsWith(this.root + path.sep)) throw new Error('Object leaves its prepared view');
      file = await open(name);
    } catch (error) {
      if (error.code === 'ENOENT') return null;
      throw error;
    }
    let info;
    try { info = await file.stat(); } catch (error) { await file.close(); throw error; }
    if (!info.isFile()) { await file.close(); throw new Error('Object is not a regular file'); }
    const etag = `${info.size}-${info.mtimeMs}`;
    if (options.onlyIf && options.onlyIf.etagMatches !== etag) {
      await file.close(); return {etag};
    }
    const offset = options.range?.offset ?? 0;
    const requested = options.range?.length ?? info.size;
    const length = Math.min(requested, info.size - offset);
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(requested) || offset < 0 || requested < 0
        || offset > info.size) { await file.close(); throw new Error('Invalid object range'); }
    const stream = length ? file.createReadStream({start: offset, end: offset + length - 1, autoClose: true}) : null;
    if (!stream) await file.close();
    const body = stream ? Readable.toWeb(stream) : new ReadableStream({start(controller) { controller.close(); }});
    const response = new Response(body);
    return {size: info.size, etag, body: response.body,
      arrayBuffer: () => response.arrayBuffer(), json: () => response.json()};
  }
}

async function catalog(bucket, request, key) {
  const headers = {'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Headers': 'Range',
    'Access-Control-Allow-Methods': 'GET, HEAD, OPTIONS', 'Access-Control-Expose-Headers': 'Content-Range, Content-Length'};
  if (request.method === 'OPTIONS') return new Response(null, {status: 204, headers});
  if (request.method !== 'GET' && request.method !== 'HEAD') return new Response(null, {status: 405, headers});
  let file = await bucket.get(key);
  if (!file) return new Response(null, {status: 404, headers});
  headers['Content-Type'] = key.endsWith('.json') ? 'application/json' : 'application/octet-stream';
  headers['Accept-Ranges'] = 'bytes';
  let length = file.size, status = 200;
  const range = request.headers.range;
  if (range) {
    await file.body.cancel();
    const match = /^bytes=([0-9]+)-([0-9]*)$/.exec(range);
    const start = match ? Number(match[1]) : NaN;
    const end = match?.[2] ? Math.min(Number(match[2]), file.size - 1) : file.size - 1;
    if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start > end || start >= file.size) {
      headers['Content-Range'] = `bytes */${file.size}`;
      return new Response(null, {status: 416, headers});
    }
    length = end - start + 1;
    headers['Content-Range'] = `bytes ${start}-${end}/${file.size}`;
    file = await bucket.get(key, {range: {offset: start, length}});
    status = 206;
  }
  headers['Content-Length'] = String(length);
  if (request.method === 'HEAD') await file.body.cancel();
  return new Response(request.method === 'HEAD' ? null : file.body, {status, headers});
}

export async function serve(root, port) {
  const bucket = new Files(await realpath(root));
  const server = http.createServer(async (request, response) => {
    try {
      const url = new URL(request.url, `http://127.0.0.1:${server.address().port}`);
      const pending = [];
      let result;
      if (url.pathname.startsWith('/cell-catalog/')) result = await catalog(bucket, request, url.pathname.slice(1));
      else result = await tiles(new Request(url, {method: request.method}), {BUCKET: bucket},
        {waitUntil(value) { pending.push(value); }}, null);
      response.writeHead(result.status, Object.fromEntries(result.headers));
      if (result.body) await pipeline(Readable.fromWeb(result.body), response);
      else response.end();
      await Promise.all(pending);
    } catch (error) {
      console.error(String(error));
      if (!response.headersSent) response.writeHead(503);
      response.end();
    }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', resolve);
  });
  return server;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [root, port] = process.argv.slice(2);
  if (!root || !/^[0-9]+$/.test(port ?? '') || Number(port) < 1 || Number(port) > 65535)
    throw new Error('Provide one prepared view and a loopback port.');
  await serve(root, Number(port));
}
