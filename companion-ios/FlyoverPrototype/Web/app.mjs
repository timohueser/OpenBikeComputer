import { measure, sample, cameraPlan, cameraAt, duration, clamp } from './track.mjs';

const C = window.Cesium;
const $ = id => document.getElementById(id);
let viewer, track, ground, plan, material, marker;
let progress = 0, playing = false, follow = true, speed = 1, pendingTiles = 0;
let lastTick = 0, lastUI = 0, lastFrame = 0, reportAt = 0, intervals = [];
let tileErrors = 0, ready = false;
const benchmark = new URLSearchParams(location.search).has('benchmark');
const startedAt = performance.now();

function report(data) {
  const payload = { ...data, playing, mode: follow ? 'follow' : 'free', progress,
    pendingTiles, tileErrors, elapsedSeconds: +(performance.now() / 1000).toFixed(1) };
  window.lastReport = payload;
  window.webkit?.messageHandlers.flyover.postMessage(payload);
  console.info('Flyover', JSON.stringify(payload));
}

function fail(error) {
  pause();
  ready = false;
  document.querySelectorAll('#controls button, #controls input').forEach(el => el.disabled = true);
  $('notice').hidden = false;
  $('message').textContent = `Cannot load the flyover. ${error.message ?? error}`;
  $('retry').hidden = false;
  report({ event: 'error', message: String(error.message ?? error) });
}
$('retry').onclick = () => location.reload();
window.addEventListener('unhandledrejection', event => fail(event.reason));
window.addEventListener('error', event => fail(event.error ?? event.message));

function pause() {
  playing = false;
  $('play').textContent = progress >= 1 ? 'Replay' : 'Play';
  if (ready) $('performance').textContent = `Paused · ${pendingTiles} tiles loading`;
  viewer?.scene.requestRender();
  report({ event: 'pause' });
}

function setMode(value) {
  follow = value;
  viewer.camera.cancelFlight();
  if (!follow) viewer.camera.lookAtTransform(C.Matrix4.IDENTITY);
  $('follow').setAttribute('aria-pressed', String(follow));
  $('free').setAttribute('aria-pressed', String(!follow));
  $('hint').textContent = follow ? 'A calm flight along the ride.' : 'Drag to pan · pinch to zoom · two fingers to tilt';
  updateScene();
  report({ event: 'camera' });
}

function updateScene() {
  if (!ready) return;
  const distance = progress * track.at(-1).distance;
  const rider = sample(ground, distance);
  marker.position = C.Cartesian3.fromDegrees(rider.lon, rider.lat, rider.height + 18);
  material.uniforms.progress = progress;
  if (follow) {
    const pose = cameraAt(plan, progress), target = sample(ground, pose.target);
    viewer.camera.lookAt(C.Cartesian3.fromDegrees(target.lon, target.lat, target.height),
      new C.HeadingPitchRange(pose.heading, pose.pitch, pose.range));
    // Keep the camera above ridges that rise between the eye and the rider.
    const eye = viewer.camera.positionCartographic;
    const floor = viewer.scene.globe.getHeight(eye);
    if (Number.isFinite(floor) && eye.height < floor + 150) {
      viewer.camera.lookAtTransform(C.Matrix4.IDENTITY);
      viewer.camera.setView({ destination: C.Cartesian3.fromRadians(eye.longitude, eye.latitude, floor + 150),
        orientation: { heading: pose.heading, pitch: pose.pitch, roll: 0 } });
    }
  }
  viewer.scene.requestRender();
}

function updateUI() {
  if (!ready) return;
  const meters = progress * track.at(-1).distance;
  $('scrub').value = progress;
  $('position').textContent = `${(meters / 1000).toFixed(1)} km`;
  $('altitude').textContent = `${Math.round(sample(track, meters).height)} m`;
  $('scrub').setAttribute('aria-valuetext', `${(meters / 1000).toFixed(1)} kilometres, ${$('altitude').textContent}`);
  drawProfile();
}

function drawProfile() {
  if (!track) return;
  const canvas = $('profile'), box = canvas.getBoundingClientRect(), ratio = Math.min(devicePixelRatio, 2);
  const width = Math.round(box.width * ratio), height = Math.round(box.height * ratio);
  if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
  const ctx = canvas.getContext('2d');
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  const style = getComputedStyle(document.documentElement), w = box.width, h = box.height;
  ctx.clearRect(0, 0, w, h);
  const heights = track.map(p => p.height), min = Math.min(...heights), max = Math.max(...heights);
  const y = elevation => h - 10 - (elevation - min) / Math.max(1, max - min) * (h - 24);
  const shape = new Path2D(); shape.moveTo(0, h);
  for (let i = 0; i <= 250; i++) shape.lineTo(i / 250 * w, y(sample(track, i / 250 * track.at(-1).distance).height));
  shape.lineTo(w, h); shape.closePath();
  ctx.fillStyle = style.getPropertyValue('--profile'); ctx.fill(shape);
  ctx.save(); ctx.beginPath(); ctx.rect(0, 0, progress * w, h); ctx.clip();
  ctx.globalAlpha = 0.5; ctx.fillStyle = style.getPropertyValue('--amber'); ctx.fill(shape); ctx.restore();
  ctx.beginPath(); ctx.moveTo(progress * w, 4); ctx.lineTo(progress * w, h);
  ctx.strokeStyle = style.getPropertyValue('--secondary'); ctx.lineWidth = 1.5; ctx.stroke();
  ctx.beginPath(); ctx.arc(progress * w, y(sample(track, progress * track.at(-1).distance).height), 4, 0, 2 * Math.PI);
  ctx.fillStyle = style.getPropertyValue('--ink'); ctx.fill();
}

async function initialize() {
  const response = await fetch('kandel.gpx');
  if (!response.ok) throw new Error('The bundled track is missing.');
  const xml = new DOMParser().parseFromString(await response.text(), 'application/xml');
  if (xml.querySelector('parsererror')) throw new Error('The track cannot be read.');
  track = measure([...xml.querySelectorAll('trkpt')].map(p => ({
    lat: Number(p.getAttribute('lat')), lon: Number(p.getAttribute('lon')),
    height: Number(p.querySelector('ele')?.textContent),
  })));
  const total = track.at(-1).distance;
  $('distance').textContent = `${(total / 1000).toFixed(1)} km · Black Forest`;
  drawProfile();

  // Explicit providers prevent Cesium from selecting its default ion cloud services.
  const [terrain, imagery] = await Promise.all([
    C.ArcGISTiledElevationTerrainProvider.fromUrl('https://elevation3d.arcgis.com/arcgis/rest/services/WorldElevation3D/Terrain3D/ImageServer'),
    C.ArcGisMapServerImageryProvider.fromUrl('https://services.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer', { enablePickFeatures: false }),
  ]);
  viewer = new C.Viewer('map', {
    terrainProvider: terrain, baseLayer: new C.ImageryLayer(imagery),
    animation: false, timeline: false, baseLayerPicker: false, geocoder: false,
    homeButton: false, sceneModePicker: false, navigationHelpButton: false,
    fullscreenButton: false, selectionIndicator: false, infoBox: false,
    scene3DOnly: true, requestRenderMode: true, maximumRenderTimeChange: Infinity,
    targetFrameRate: 60, useBrowserRecommendedResolution: false,
    contextOptions: { webgl: { antialias: false } },
  });
  viewer.resolutionScale = Math.min(devicePixelRatio, 1.5) / devicePixelRatio;
  viewer.scene.globe.maximumScreenSpaceError = 3;
  viewer.scene.globe.tileCacheSize = 128;
  viewer.scene.globe.depthTestAgainstTerrain = true;
  viewer.scene.globe.enableLighting = false;
  viewer.scene.fog.enabled = true;
  viewer.scene.screenSpaceCameraController.minimumZoomDistance = 80;
  viewer.scene.globe.tileLoadProgressEvent.addEventListener(count => {
    pendingTiles = count;
    if (ready && !playing) $('performance').textContent = `Paused · ${count} tiles loading`;
  });
  for (const provider of [terrain, imagery]) provider.errorEvent.addEventListener(error => {
    tileErrors++;
    $('hint').textContent = 'Some map tiles failed to load. Check your connection.';
    report({ event: 'tile-error', message: error.message });
  });
  viewer.scene.renderError.addEventListener((scene, error) => fail(error));
  const first = track[0];
  viewer.camera.lookAt(C.Cartesian3.fromDegrees(first.lon, first.lat, first.height), new C.HeadingPitchRange(0, -0.65, 2400));

  $('message').textContent = 'Placing the route on the terrain…';
  const count = Math.min(1200, Math.ceil(total / 60));
  ground = Array.from({ length: count + 1 }, (_, i) => sample(track, total * i / count));
  const cartographic = ground.map(p => C.Cartographic.fromDegrees(p.lon, p.lat));
  // One bounded detail level gives the route stable heights while camera tiles refine.
  await C.sampleTerrain(terrain, 13, cartographic, true);
  ground.forEach((p, i) => {
    if (!Number.isFinite(cartographic[i].height)) throw new Error('Terrain height is unavailable for this ride.');
    p.height = cartographic[i].height;
  });
  const positions = ground.map(p => C.Cartesian3.fromDegrees(p.lon, p.lat, p.height + 12));
  material = new C.Material({ fabric: {
    type: 'RideProgress', uniforms: { progress: 0 },
    source: `czm_material czm_getMaterial(czm_materialInput materialInput) {
      czm_material m = czm_getDefaultMaterial(materialInput);
      float ridden = step(materialInput.st.s, progress);
      m.diffuse = mix(vec3(0.95, 0.94, 0.89), vec3(0.96, 0.66, 0.11), ridden);
      m.alpha = mix(0.45, 1.0, ridden);
      return m;
    }`,
  }});
  viewer.scene.primitives.add(new C.Primitive({
    geometryInstances: new C.GeometryInstance({ geometry: new C.PolylineGeometry({
      positions, width: 5, arcType: C.ArcType.NONE, vertexFormat: C.PolylineMaterialAppearance.VERTEX_FORMAT,
    }) }),
    appearance: new C.PolylineMaterialAppearance({ material }),
  }));
  const points = viewer.scene.primitives.add(new C.PointPrimitiveCollection());
  marker = points.add({ position: positions[0], pixelSize: 12, color: C.Color.WHITE,
    outlineColor: C.Color.fromCssColorString('#1c1b14'), outlineWidth: 3,
    disableDepthTestDistance: Number.POSITIVE_INFINITY });
  plan = cameraPlan(track);
  ready = true;
  document.querySelectorAll('button, input').forEach(el => el.disabled = false);
  $('notice').hidden = true;
  updateScene(); updateUI();
  report({ event: 'ready', loadSeconds: +((performance.now() - startedAt) / 1000).toFixed(1), routePoints: ground.length });

  viewer.scene.preUpdate.addEventListener(() => {
    const now = performance.now(), elapsed = lastTick ? (now - lastTick) / 1000 : 0;
    lastTick = now;
    if (!playing) return;
    progress = clamp(progress + elapsed * speed / duration, 0, 1);
    updateScene();
    if (now - lastUI > 100) { updateUI(); lastUI = now; }
    if (progress >= 1) { pause(); updateUI(); }
  });
  viewer.scene.postRender.addEventListener(() => {
    const now = performance.now();
    if (playing && lastFrame) intervals.push(now - lastFrame);
    lastFrame = playing ? now : 0;
    if (now - reportAt < 5000) return;
    const sorted = intervals.toSorted((a, b) => a - b);
    const fps = intervals.length ? 1000 * intervals.length / intervals.reduce((a, b) => a + b) : 0;
    const p95 = sorted[Math.floor(sorted.length * 0.95)] ?? 0;
    $('performance').textContent = playing ? `${fps.toFixed(0)} fps · p95 ${p95.toFixed(0)} ms · ${pendingTiles} tiles` : `Paused · ${pendingTiles} tiles loading`;
    report({ event: 'frames', fps: +fps.toFixed(1), p95ms: +p95.toFixed(1),
      samples: intervals.length, targetFPS: viewer.targetFrameRate,
      pixels: [viewer.scene.drawingBufferWidth, viewer.scene.drawingBufferHeight] });
    reportAt = now; intervals = [];
  });
  if (benchmark) {
    // Let the first view settle, then measure a complete flight with streamed terrain.
    setTimeout(() => { if (ready && !playing && progress === 0) $('play').click(); }, 8000);
  }
}

$('play').onclick = () => {
  if (playing) { pause(); return; }
  if (progress >= 1) progress = 0;
  playing = true; lastTick = performance.now(); lastFrame = 0; intervals = []; reportAt = lastTick;
  $('play').textContent = 'Pause'; updateScene(); updateUI(); report({ event: 'play' });
};
$('follow').onclick = () => setMode(true);
$('free').onclick = () => setMode(false);
$('overview').onclick = () => {
  setMode(false);
  viewer.camera.flyToBoundingSphere(C.BoundingSphere.fromPoints(ground.map(p => C.Cartesian3.fromDegrees(p.lon, p.lat, p.height))), {
    duration: 0.8, offset: new C.HeadingPitchRange(0, -0.85, 0),
  });
};
$('map').addEventListener('pointerdown', () => { if (ready && follow) setMode(false); });
$('scrub').addEventListener('input', () => {
  progress = Number($('scrub').value); pause(); updateScene(); updateUI();
});
$('rate').onclick = () => { speed = speed === 1 ? 0.5 : 1; $('rate').textContent = `${speed}×`; };
$('fps').onclick = () => {
  if (!viewer) return;
  viewer.targetFrameRate = viewer.targetFrameRate === 60 ? 30 : 60;
  intervals = []; lastFrame = 0;
  $('fps').textContent = `${viewer.targetFrameRate} fps target`;
};
document.addEventListener('visibilitychange', () => { if (document.hidden) pause(); });
new ResizeObserver(drawProfile).observe($('profile'));
matchMedia('(prefers-color-scheme: dark)').addEventListener('change', drawProfile);
window.flyover = { pause };
initialize().catch(fail);
