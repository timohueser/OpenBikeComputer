import { mkdirSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from 'vite';

const BAKES = join(homedir(), '.cache/obc/trail-view');

// The bake output is served as static files: /<preset>/meta.json and so on. A POST to
// /__shot/<name>.png writes a side-by-side into <bakes>/shots/.
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
  ],
});
