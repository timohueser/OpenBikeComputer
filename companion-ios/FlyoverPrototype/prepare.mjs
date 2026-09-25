import { cp, rm } from 'node:fs/promises';

await rm('Web/cesium', { recursive: true, force: true });
await cp('node_modules/cesium/Build/Cesium', 'Web/cesium', { recursive: true });
await cp('node_modules/cesium/LICENSE.md', 'Web/cesium/LICENSE.md');
await cp('node_modules/cesium/ThirdParty.json', 'Web/cesium/ThirdParty.json');
