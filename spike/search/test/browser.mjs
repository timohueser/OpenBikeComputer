import {chromium} from 'playwright';
import assert from 'node:assert/strict';
import {mkdir,writeFile} from 'node:fs/promises';

const executablePath=process.env.CHROMIUM_PATH;
const browser=await chromium.launch(executablePath?{executablePath}:{});
const context=await browser.newContext({viewport:{width:1440,height:1000}});
const page=await context.newPage(),errors=[];
page.on('pageerror',e=>errors.push(e.message));
await mkdir('test-output',{recursive:true});
const measurements={};
try {
  await page.goto('http://localhost:8780');
  await page.waitForFunction(()=>window.searchSpike);
  await page.locator('#sample-route').focus();
  await page.keyboard.press('Tab');
  assert.equal(await page.evaluate(()=>document.activeElement.id),'open-gpx');
  const chooser=page.waitForEvent('filechooser');
  await page.keyboard.press('Enter');
  assert.ok(await chooser);
  console.log('PASS keyboard GPX picker');
  for(const [q,expect,kind='bakery'] of [
    ['bakeries in Munich',{near:'Munich',in:true}],
    ['bakeries at the end of day three',{day:3,part:'end'}],
    ['bakeries in the first half of the route',{scope:'route',part:'first_half'}],
    ['bakeries near me',{scope:'here'}],
    ['Bäckereien in München',{near:'München',in:true}],
    ['pizza', {scope:'view'}, 'pizza'],
    ['Döner', {scope:'view'}, 'kebab'],
    ['pizza in Munich', {near:'Munich',in:true}, 'pizza'],
    ['Döner in Teningen', {near:'Teningen',in:true}, 'kebab'],
    ['pizza at the end of day three', {day:3,part:'end'}, 'pizza'],
    ['Döner near me', {scope:'here'}, 'kebab'],
  ]) {
    const r=await page.evaluate(q=>window.searchSpike.parser('parse',{q}),q);
    assert.equal(r.type,'places',q);assert.deepEqual(r.what,[kind],q);
    for(const [k,v]of Object.entries(expect))assert.equal(r.where[k],v,q);
  }
  console.log('PASS browser mmBERT search interpretations');
  await page.locator('#query').fill('bakeries in Munich');
  await page.locator('#search-form').evaluate(form=>form.requestSubmit());
  await page.waitForFunction(()=>window.searchSpike.output?.area==='In München');
  assert.ok(await page.evaluate(()=>window.searchSpike.results.length>0&&window.searchSpike.results.every(p=>p.city==='München')));
  assert.ok(await page.evaluate(()=>Math.abs(window.searchSpike.map.getCenter().lng-11.58)<.3),'Explicit city moves the map');
  console.log('PASS Germany server search outside the offline region');
  const t=Date.now();
  await page.locator('#download').click();
  await page.waitForFunction(()=>document.querySelector('#offline-state').textContent==='Ready on this browser',null,{timeout:120000});
  measurements.installMs=Date.now()-t;
  console.log('PASS browser OPFS download');
  const queries=['bakery','Kandel','Freiburg','Kaiser Joseph Straße 242 Freiburg'];
  const times=[];
  for(const q of queries) {
    const r=await page.evaluate(async q=>window.searchSpike.offline('search',{input:{q,view:[7.77,47.965,7.96,48.06]}}),q);
    assert.ok(r.results.length,q);times.push({q,ms:r.elapsed,first:r.results[0].name,precision:r.results[0].precision});
  }
  measurements.offlineFirst=times;
  assert.equal(times[1].first,'Kandel');assert.equal(times[3].precision,'house');
  await context.setOffline(true);
  await page.reload();
  await page.waitForFunction(()=>window.searchSpike);
  const offline=await page.evaluate(()=>window.searchSpike.offline('status'));
  assert.equal(offline.installed,true);
  await page.locator('#query').fill('bakery');
  await page.locator('#search-form').evaluate(form=>form.requestSubmit());
  await page.waitForFunction(()=>window.searchSpike.results.length>0);
  assert.ok(await page.evaluate(()=>window.searchSpike.results.every(p=>p.served==='Offline')));
  const parsed=await page.evaluate(()=>window.searchSpike.parser('parse',{q:'bakeries in Munich'}));
  assert.equal(parsed.where.near,'Munich');
  measurements.offlineMatching=[];
  for(const [q,check] of [
    ['Habsburgerstr. 10 Freiburg',r=>r[0]?.source==='w154330310'],
    ...['Media Markt','NediaMarkt','Mdeia Mrkt'].map(q=>[q,r=>r[0]?.source==='n809686332']),
    ['Feldberg',r=>r[0]?.source==='n26862857'],
    ['Kandel',r=>r.some(p=>p.source==='n1591343465'&&p.kind==='pass')&&r.filter(p=>p.kind==='street'&&p.name==='Kandel'&&p.distance<30).length===1],
    ['pizza',r=>r.length>0&&r.some(p=>p.why.match.includes('OSM cuisine tag'))],
    ['Döner',r=>r.length>0&&r.some(p=>p.why.match.includes('OSM cuisine tag'))],
  ]) {
    const r=await page.evaluate(q=>window.searchSpike.offline('search',{input:{q,view:[7.77,47.965,7.96,48.06]}}),q);
    assert.ok(check(r.results),q);
    measurements.offlineMatching.push({q,ms:r.elapsed});
  }
  const offlineBranches=await page.evaluate(async()=>{
    const r=await window.searchSpike.offline('search',{input:{q:'Media Markt',view:[7.76,48.08,7.86,48.16]}});
    return r.results.filter(p=>p.kind==='electronics'&&p.name.replace(/\s/g,'').toLowerCase()==='mediamarkt');
  });
  assert.deepEqual(offlineBranches.slice(0,3).map(p=>p.source),['n414000115','n809686332','w143665549']);
  assert.ok(offlineBranches.every((p,i)=>!i||offlineBranches[i-1].distance<=p.distance));
  console.log('PASS full offline reload, local search and cached mmBERT');
  const stats=[];
  for(let i=0;i<12;i++)stats.push(await page.evaluate(async q=>(await window.searchSpike.offline('search',{input:{q,view:[7.77,47.965,7.96,48.06]}})).elapsed,queries[i%queries.length]));
  stats.sort((a,b)=>a-b);measurements.offlineWarm={medianMs:stats[Math.floor(stats.length/2)],p95Ms:stats[Math.ceil(stats.length*.95)-1]};
  await context.setOffline(false);
  await page.locator('#online').check();
  await page.locator('[data-view="freiburg"]').click();
  await page.locator('#query').fill('bakery');
  await page.locator('#search-form').evaluate(form=>form.requestSubmit());
  await page.waitForFunction(()=>window.searchSpike.output?.results?.some(p=>p.served==='Offline + server'));
  const ids=await page.evaluate(()=>window.searchSpike.results.map(p=>p.source));
  assert.equal(ids.length,new Set(ids).size);
  console.log('PASS online/offline merge without duplicate source IDs');
  await page.locator('#sample-route').click();
  await page.locator('#query').fill('bakeries in the first half of the route');
  await page.locator('#search-form').evaluate(form=>form.requestSubmit());
  await page.waitForFunction(()=>window.searchSpike.output?.area==='First half of the route');
  assert.ok(await page.evaluate(()=>window.searchSpike.results.length>0&&window.searchSpike.results.every(p=>p.position.along<=p.position.total/2)));
  console.log('PASS route corridor and first-half filter');
  await page.locator('[data-view="freiburg"]').click();
  await page.locator('#query').fill('bakery');
  await page.locator('#search-form').evaluate(form=>form.requestSubmit());
  await page.waitForFunction(()=>window.searchSpike.output?.area==='Visible map area');
  await page.locator('#clear-route').click();
  await page.evaluate(()=>window.searchSpike.map.setView([48.12,7.81],13));
  await page.locator('#query').fill('Media Markt');
  await page.locator('#search-form').evaluate(form=>form.requestSubmit());
  await page.waitForFunction(()=>window.searchSpike.output?.request?.name==='Media Markt'&&window.searchSpike.results[0]?.source==='n414000115');
  const mergedBranches=await page.evaluate(()=>window.searchSpike.results.filter(p=>p.kind==='electronics'&&p.name.replace(/\s/g,'').toLowerCase()==='mediamarkt'));
  assert.deepEqual(mergedBranches.slice(0,3).map(p=>p.source),['n414000115','n809686332','w143665549']);
  assert.ok(mergedBranches.every((p,i)=>!i||mergedBranches[i-1].distance<=p.distance));
  assert.ok(mergedBranches.some(p=>p.served==='Offline + server'));
  console.log('PASS equivalent branches rank by distance offline and after merging');
  console.log('PASS split-name search through the visible search field');
  const settledTiles=()=>[...document.querySelectorAll('.leaflet-tile')].every(img=>img.complete&&img.naturalWidth>0&&getComputedStyle(img).opacity==='1');
  await page.waitForFunction(settledTiles);
  await page.screenshot({path:'test-output/desktop.png',fullPage:true,animations:'disabled'});
  await page.setViewportSize({width:390,height:844});
  await page.evaluate(()=>window.searchSpike.map.invalidateSize());
  await page.waitForFunction(settledTiles);
  await page.screenshot({path:'test-output/mobile.png',fullPage:true,animations:'disabled'});
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,'No horizontal overflow');
  assert.deepEqual(errors,[]);
  await page.locator('#query').fill('');
  assert.equal(await page.evaluate(()=>window.searchSpike.results.length),0,'Clearing input clears stale results');
  measurements.browserStorage=await page.evaluate(()=>navigator.storage.estimate());
  await writeFile('test-output/browser.json',JSON.stringify(measurements,null,2));
  console.log(JSON.stringify(measurements,null,2));
} finally {await browser.close();}
