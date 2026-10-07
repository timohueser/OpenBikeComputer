import {build} from 'esbuild';
import {mkdir, writeFile} from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {licenseNotices} from '../../builder/web/vite/third-party-licenses.ts';

const output = process.argv[2] && path.resolve(process.argv[2]);
if (!output) throw new Error('Usage: node planner/search/native-build.mjs OUTPUT');
await mkdir(output, {recursive: true});
const result = await build({
  entryPoints: [fileURLToPath(new URL('./native.mjs', import.meta.url))], bundle: true,
  format: 'iife', globalName: 'PlannerNative', target: 'safari17', metafile: true,
  outfile: path.join(output, 'native.js'),
});
const inputs = Object.keys(result.metafile.inputs);
await writeFile(path.join(output, 'third-party-licenses.txt'),
  licenseNotices(inputs.map(file => path.resolve(file)), 'OpenBikeComputer native search'));
