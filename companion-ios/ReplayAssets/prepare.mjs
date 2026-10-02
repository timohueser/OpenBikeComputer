import { cp, mkdir, rm, readFile, writeFile } from 'node:fs/promises';
import ts from 'typescript';

const source = new URL('node_modules/cesium/', import.meta.url);
const destination = new URL('../Packages/OBCKit/Sources/OBCUI/Resources/Replay/cesium/', import.meta.url);
await rm(destination, { recursive: true, force: true });
await mkdir(destination, { recursive: true });
await cp(new URL('Build/Cesium/', source), destination, { recursive: true });
for (const name of ['LICENSE.md', 'ThirdParty.json']) {
  await cp(new URL(name, source), new URL(name, destination));
}

// Use the web planner's palette, POI categories, and style in the native renderer.
const cache = new URL('node_modules/.cache/', import.meta.url);
await mkdir(cache, { recursive: true });
for (const name of ['poi-kinds', 'map-style']) {
  let text = await readFile(new URL(`../../builder/app/src/lib/planner/${name}.ts`, import.meta.url), 'utf8');
  text = text.replaceAll('"./map-data"', '"./obc-map-data.mjs"').replaceAll('"./poi-kinds"', '"./poi-kinds.mjs"');
  await writeFile(new URL(`${name}.mjs`, cache), ts.transpileModule(text, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  }).outputText);
}
await writeFile(new URL('obc-map-data.mjs', cache), `export const BASEMAP_URL='__BASEMAP__',GLYPHS_URL='__GLYPHS__',SPRITES_URL='__SPRITES__',MAP_BOUNDS=null,TERRAIN_ATTRIBUTION='__TERRAIN_ATTRIBUTION__';`);
const { mapStyle } = await import(new URL('map-style.mjs', cache).href);
const { poiKinds } = await import(new URL('poi-kinds.mjs', cache).href);
const maps = new URL('../Packages/OBCKit/Sources/OBCUI/Resources/Map/', import.meta.url);
await mkdir(maps, { recursive: true });
await writeFile(new URL('poi-kinds.json', maps), JSON.stringify(poiKinds));
for (const theme of ['light', 'dark']) {
  const style = mapStyle(theme, '__TERRAIN__', '__CONTOURS__');
  // The web renderer generates contours from DEM tiles; native uses the DEM hillshade.
  style.layers = style.layers.filter(layer => layer.source !== 'contours');
  delete style.sources.contours;
  style.sources.networks = { type: 'geojson', data: { type: 'FeatureCollection', features: [] },
    attribution: '<a href="https://www.openstreetmap.org/copyright">Route networks © OpenStreetMap contributors</a>' };
  const colors = theme === 'dark' ? ['#b0b8be','#a4cf67','#79b7f1','#c49de0'] : ['#626a70','#4f8b24','#2368b5','#7c519c'];
  const color = ['match', ['get', 'rank'], 1, colors[1], 2, colors[2], 3, colors[3], colors[0]];
  const before = style.layers.findIndex(layer => layer.type === 'symbol');
  style.layers.splice(before, 0, { id: 'planner-networks', type: 'line', source: 'networks',
    layout: { 'line-join': 'round' }, paint: { 'line-color': color, 'line-opacity': 0.8,
      'line-width': ['interpolate',['linear'],['zoom'],6,1.2,10,1.8,13,3.2,17,5] } },
    { id: 'planner-network-labels', type: 'symbol', source: 'networks', minzoom: 11,
      filter: ['!=', ['get','ref'], ''],
      layout: { 'symbol-placement': 'line', 'symbol-spacing': 350, 'text-field': ['get','ref'],
        'text-font': ['Noto Sans Medium'], 'text-size': 11, 'text-offset': [0,0.8] },
      paint: { 'text-color': color, 'text-halo-color': theme === 'dark' ? '#181d19' : '#ffffff', 'text-halo-width': 2 } });
  style.layers.push({ id: 'planner-poi-highlight', type: 'circle', source: 'basemap', 'source-layer': 'pois', minzoom: 13,
    filter: ['in', ['get','kind'], ['literal',[]]],
    paint: { 'circle-radius': 19, 'circle-color': 'transparent', 'circle-stroke-color': '#e1ac42', 'circle-stroke-width': 3 } });
  style.sources['highlighted-places'] = { type: 'geojson', data: { type: 'FeatureCollection', features: [] } };
  style.layers.push({ id: 'highlighted-place-ring', type: 'circle', source: 'highlighted-places', maxzoom: 13,
    paint: { 'circle-radius': 19, 'circle-color': 'transparent', 'circle-stroke-color': '#e1ac42', 'circle-stroke-width': 3 } });
  const icons = structuredClone(style.layers.find(layer => layer.id === 'planner-poi-icons'));
  Object.assign(icons, { id: 'highlighted-place-icons', source: 'highlighted-places', minzoom: 0, maxzoom: 13 });
  delete icons['source-layer']; delete icons.filter;
  icons.layout['icon-allow-overlap'] = true;
  icons.layout['icon-ignore-placement'] = true;
  icons.layout['icon-image'] = ['concat', 'poi-', ['get','category'], `-${theme}`];
  style.layers.push(icons);
  await writeFile(new URL(theme + '.json', maps), JSON.stringify(style));
}
