import { createReadStream, mkdirSync, statSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { extname, join, sep } from 'node:path';
import { defineConfig } from 'vite';

const BAKES = join(homedir(), '.cache/obc/trail-view');
const TYPES: Record<string, string> = { '.json': 'application/json', '.png': 'image/png', '.jpg': 'image/jpeg' };

// The bake output is read from disk at each request, so a spot baked while the server runs shows
// at once: /<preset>/meta.json and so on, and the tune's /status.json. A POST to
// /__shot/<name>.png writes a side-by-side into <bakes>/shots/.
export default defineConfig({
  publicDir: false,
  // A missing bake file is a 404, not index.html.
  appType: 'mpa',
  // The page uses the planner's sun model from the repository. An inline tsconfig keeps esbuild
  // from reading builder/web's, which extends a package installed only there.
  esbuild: { tsconfigRaw: '{}' },
  resolve: { alias: { 'planner-sun': join(import.meta.dirname, '../../builder/web/src/lib/planner/layers/sun.ts') } },
  server: { port: 5180, strictPort: true, fs: { allow: [join(import.meta.dirname, '../..')] } },
  plugins: [
    {
      name: 'bakes',
      configureServer(server) {
        server.middlewares.use((request, response, next) => {
          const path = join(BAKES, decodeURIComponent((request.url ?? '/').split('?')[0]));
          if (request.method !== 'GET' || !path.startsWith(BAKES + sep) || !statSync(path, { throwIfNoEntry: false })?.isFile()) return next();
          response.setHeader('Content-Type', TYPES[extname(path)] ?? 'application/octet-stream');
          createReadStream(path).pipe(response);
        });
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
  ],
});
