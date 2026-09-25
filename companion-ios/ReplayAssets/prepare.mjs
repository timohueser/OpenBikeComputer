import { cp, mkdir, rm } from 'node:fs/promises';

const source = new URL('node_modules/cesium/', import.meta.url);
const destination = new URL('../Packages/OBCKit/Sources/OBCUI/Resources/Replay/cesium/', import.meta.url);
await rm(destination, { recursive: true, force: true });
await mkdir(destination, { recursive: true });
await cp(new URL('Build/Cesium/', source), destination, { recursive: true });
for (const name of ['LICENSE.md', 'ThirdParty.json']) {
  await cp(new URL(name, source), new URL(name, destination));
}
