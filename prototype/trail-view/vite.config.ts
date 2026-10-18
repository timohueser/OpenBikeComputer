import { mkdirSync, writeFileSync } from 'node:fs';
import type { ServerResponse } from 'node:http';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from 'vite';
import { comment, comments, known, newest, regions, rounds, status } from './tune/review.mjs';

const BAKES = join(homedir(), '.cache/obc/trail-view');

function send(response: ServerResponse, code: number, body?: unknown): void {
  response.statusCode = code;
  response.setHeader('Content-Type', 'application/json');
  response.end(body === undefined ? undefined : JSON.stringify(body));
}

// The bake output is served as static files: /<preset>/meta.json and so on, and the review's
// shots as /review/<short>/<n>/<file>.png. A POST to /__shot/<name>.png writes a side-by-side into
// <bakes>/shots/. /__review and /__comment/<short> read and write the review (tune/review.mjs).
export default defineConfig({
  publicDir: BAKES,
  // The page uses the planner's sun model from the repository. An inline tsconfig keeps esbuild
  // from reading builder/web's, which extends a package installed only there.
  esbuild: { tsconfigRaw: '{}' },
  resolve: { alias: { 'planner-sun': join(import.meta.dirname, '../../builder/web/src/lib/planner/layers/sun.ts') } },
  server: { port: 5180, strictPort: true, fs: { allow: [join(import.meta.dirname, '../..')] } },
  plugins: [
    {
      name: 'save-shots',
      configureServer(server) {
        server.middlewares.use('/__shot/', (request, response) => {
          const name = (request.url ?? '').slice(1);
          if (request.method !== 'POST' || !/^[\w.-]+\.png$/.test(name)) {
            response.statusCode = 400;
            response.end();
            return;
          }
          const chunks: Buffer[] = [];
          request.on('data', (chunk: Buffer) => chunks.push(chunk));
          request.on('end', () => {
            mkdirSync(join(BAKES, 'shots'), { recursive: true });
            writeFileSync(join(BAKES, 'shots', name), Buffer.concat(chunks));
            response.end('saved');
          });
        });
      },
    },
    {
      name: 'review',
      configureServer(server) {
        server.middlewares.use('/__review', (request, response) => {
          const short = (request.url ?? '/').slice(1);
          if (request.method !== 'GET') return send(response, 405);
          if (!short) return send(response, 200, regions().map((r) => ({ ...r, status: status(r.short), round: newest(r.short) })));
          if (!known(short)) return send(response, 404);
          send(response, 200, { rounds: rounds(short), comments: comments(short) });
        });
        server.middlewares.use('/__comment/', (request, response) => {
          const short = (request.url ?? '').slice(1);
          // JSON only: another origin cannot send it without a preflight, and the preflight fails.
          if (request.method !== 'POST' || !request.headers['content-type']?.startsWith('application/json')) return send(response, 415);
          if (!known(short)) return send(response, 404);
          let body = '';
          request.on('data', (chunk: Buffer) => (body += chunk));
          request.on('end', () => {
            let posted: { text?: unknown; spot?: unknown; approve?: unknown };
            try {
              posted = JSON.parse(body);
            } catch {
              return send(response, 400);
            }
            const { text, spot, approve } = posted;
            if (typeof text !== 'string' || !text.trim() || text.length > 20_000) return send(response, 400);
            if (spot !== undefined && (typeof spot !== 'string' || !/^[\w.-]+$/.test(spot))) return send(response, 400);
            send(response, 200, comment(short, { spot, text: text.trim(), by: 'owner', approve: approve === true }));
          });
        });
      },
    },
  ],
});
