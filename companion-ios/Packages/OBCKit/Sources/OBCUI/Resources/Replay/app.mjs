import { normalizeTrack, segments, sample, cameraPlan, cameraAt, adjustedPose, terrainSamples, Playback, clamp } from './track.mjs';
import { createProviders } from './providers.mjs';

const C = window.Cesium;
let session;
const send = data => window.webkit?.messageHandlers.replay?.postMessage(data);

class Replay {
  constructor(command) {
    this.track = normalizeTrack(command.track?.points);
    this.state = new Playback(this.track.at(-1).distance, command.track.durationSeconds);
    this.reducedMotion = command.reducedMotion === true;
    this.cancelled = false;
    this.ready = false;
    this.pendingTiles = 0;
    this.tileErrors = 0;
    this.disposers = [];
    this.lastProgress = -Infinity;
    this.frameTimes = [];
    this.metricsAt = performance.now();
    this.timer = setTimeout(() => this.fail('The map did not load. Check your connection and try again.'), 30000);
  }
  async load() {
    if (!C) throw new Error('The bundled replay renderer is unavailable.');
    const { terrain, imagery } = await createProviders(C);
    if (this.cancelled) return;
    const viewer = this.viewer = new C.Viewer('map', {
      terrainProvider: terrain, baseLayer: new C.ImageryLayer(imagery),
      animation: false, timeline: false, baseLayerPicker: false, geocoder: false,
      homeButton: false, sceneModePicker: false, navigationHelpButton: false,
      fullscreenButton: false, selectionIndicator: false, infoBox: false,
      scene3DOnly: true, requestRenderMode: true, maximumRenderTimeChange: Infinity,
      targetFrameRate: 60, useBrowserRecommendedResolution: false,
      contextOptions: { webgl: { antialias: false } },
    });
    viewer.clock.shouldAnimate = false;
    viewer.resolutionScale = Math.min(devicePixelRatio, 1.5) / devicePixelRatio;
    viewer.scene.globe.maximumScreenSpaceError = 3;
    viewer.scene.globe.tileCacheSize = 128;
    viewer.scene.globe.depthTestAgainstTerrain = true;
    viewer.scene.globe.enableLighting = false;
    viewer.scene.screenSpaceCameraController.enableInputs = false;
    this.disposers.push(viewer.scene.globe.tileLoadProgressEvent.addEventListener(count => { this.pendingTiles = count; }));
    for (const provider of [terrain, imagery]) {
      this.disposers.push(provider.errorEvent.addEventListener(() => {
        this.tileErrors++;
      }));
    }
    this.disposers.push(viewer.scene.renderError.addEventListener(() => this.fail('The map renderer stopped. Try again.')));
    const first = this.track[0];
    viewer.camera.lookAt(C.Cartesian3.fromDegrees(first.lon, first.lat, first.height ?? 0), new C.HeadingPitchRange(0, -0.65, 2400));
    this.ground = terrainSamples(this.track);
    for (let index = 0; index < this.ground.length; index += 64) {
      const batch = this.ground.slice(index, index + 64);
      const positions = batch.map(point => C.Cartographic.fromDegrees(point.lon, point.lat));
      await C.sampleTerrain(terrain, 13, positions, true);
      if (this.cancelled) return;
      batch.forEach((point, i) => {
        if (!Number.isFinite(positions[i].height)) throw new Error('Terrain height is unavailable for this ride.');
        point.height = positions[i].height;
      });
    }
    this.parts = new Map(segments(this.ground).map(part => [part[0].segment, part]));
    this.plan = cameraPlan(this.track, this.state.duration);
    this.addRoute();
    this.installGestures();
    this.draw();
    let settledAt = 0;
    this.loadPoll = setInterval(() => {
      if (this.cancelled) return;
      viewer.scene.requestRender();
      if (!viewer.scene.globe.tilesLoaded) { settledAt = 0; return; }
      if (!settledAt) settledAt = performance.now();
      if (performance.now() - settledAt < 300) return;
      clearInterval(this.loadPoll);
      clearTimeout(this.timer);
      this.ready = true;
      send({ type: 'ready' });
      this.progress(true);
    }, 100);
    this.disposers.push(viewer.scene.postRender.addEventListener(() => this.recordFrame()));
  }
  addRoute() {
    this.material = new C.Material({ fabric: {
      type: 'ReplayProgress', uniforms: { progress: 0 },
      source: `czm_material czm_getMaterial(czm_materialInput materialInput) {
        czm_material m = czm_getDefaultMaterial(materialInput);
        float ridden = step(materialInput.st.s, progress);
        m.diffuse = mix(vec3(0.95, 0.94, 0.89), vec3(0.96, 0.66, 0.11), ridden);
        m.alpha = mix(0.45, 1.0, ridden);
        return m;
      }`,
    }});
    const geometryInstances = [];
    const allPositions = [];
    for (const part of this.parts.values()) {
      const positions = part.map(point => C.Cartesian3.fromDegrees(point.lon, point.lat, point.height + 12));
      allPositions.push(...positions);
      if (positions.length < 2 || part[0].distance === part.at(-1).distance) continue;
      const geometry = C.PolylineGeometry.createGeometry(new C.PolylineGeometry({
        positions, width: 5, arcType: C.ArcType.NONE, vertexFormat: C.PolylineMaterialAppearance.VERTEX_FORMAT,
      }));
      if (!geometry) continue;
      // Cesium encodes vertex index in s; replace it with canonical ride distance.
      const st = geometry.attributes.st.values;
      for (let index = 0; index < st.length; index += 2) {
        const vertex = Math.round(st[index] * (part.length - 1));
        st[index] = part[vertex].distance / Math.max(1, this.state.total);
      }
      geometryInstances.push(new C.GeometryInstance({ geometry }));
    }
    if (geometryInstances.length) this.viewer.scene.primitives.add(new C.Primitive({
      geometryInstances, asynchronous: false,
      appearance: new C.PolylineMaterialAppearance({ material: this.material }),
    }));
    this.bounds = C.BoundingSphere.fromPoints(allPositions);
    const points = this.viewer.scene.primitives.add(new C.PointPrimitiveCollection());
    this.marker = points.add({ position: allPositions[0], pixelSize: 12, color: C.Color.WHITE,
      outlineColor: C.Color.fromCssColorString('#1c1b14'), outlineWidth: 3,
      disableDepthTestDistance: Number.POSITIVE_INFINITY });
  }
  draw() {
    if (!this.viewer || !this.plan || this.cancelled) return;
    const { distance, mode } = this.state;
    const rider = sample(this.ground, distance);
    if (this.lastSegment !== undefined && this.lastSegment !== rider.segment && this.state.playing && mode !== 'overview' && !this.reducedMotion) {
      this.gapAnimation?.cancel();
      this.gapAnimation = this.viewer.canvas.animate([{ opacity: 0.25 }, { opacity: 1 }], { duration: 250, easing: 'ease-out' });
    }
    this.lastSegment = rider.segment;
    this.marker.position = C.Cartesian3.fromDegrees(rider.lon, rider.lat, rider.height + 18);
    this.material.uniforms.progress = distance / Math.max(1, this.state.total);
    if (mode !== 'overview') {
      let offsets = this.state.offsets;
      if (this.reset) {
        const t = clamp((performance.now() - this.reset.started) / 450, 0, 1), remaining = (1 - t) ** 3;
        offsets = { heading: this.reset.offsets.heading * remaining, pitch: this.reset.offsets.pitch * remaining,
          range: 1 + (this.reset.offsets.range - 1) * remaining };
        if (t === 1) this.reset = null;
      }
      const pose = adjustedPose(cameraAt(this.plan, distance), offsets);
      const target = sample(this.parts.get(pose.segment), pose.target);
      const center = C.Cartesian3.fromDegrees(target.lon, target.lat, target.height);
      this.viewer.camera.lookAt(center, new C.HeadingPitchRange(pose.heading, pose.pitch, pose.range));
      // Keep the eye above visible ridges without detaching it from the rider.
      const eye = this.viewer.camera.positionCartographic, floor = this.viewer.scene.globe.getHeight(eye);
      if (Number.isFinite(floor) && eye.height < floor + 150) {
        const destination = C.Cartesian3.fromRadians(eye.longitude, eye.latitude, floor + 150);
        const direction = C.Cartesian3.normalize(C.Cartesian3.subtract(center, destination, new C.Cartesian3()), new C.Cartesian3());
        const right = C.Cartesian3.normalize(C.Cartesian3.cross(direction, C.Cartesian3.normalize(destination, new C.Cartesian3()), new C.Cartesian3()), new C.Cartesian3());
        const up = C.Cartesian3.cross(right, direction, new C.Cartesian3());
        this.viewer.camera.lookAtTransform(C.Matrix4.IDENTITY);
        this.viewer.camera.setView({ destination, orientation: { direction, up } });
      }
    }
    this.viewer.scene.requestRender();
  }
  progress(force = false) {
    const now = performance.now();
    if (now - this.lastProgress < 100) {
      if (force && !this.progressTimer) this.progressTimer = setTimeout(() => {
        this.progressTimer = null;
        if (!this.cancelled) this.progress(true);
      }, 100 - (now - this.lastProgress));
      return;
    }
    clearTimeout(this.progressTimer); this.progressTimer = null;
    this.lastProgress = now;
    const { distance, playing, mode } = this.state;
    send({ type: 'progress', distance, playing, mode, cameraState: this.state.cameraSnapshot() });
  }
  schedule() {
    if (this.frame || this.cancelled || (!this.state.playing && !this.reset)) return;
    this.frame = requestAnimationFrame(now => {
      this.frame = 0;
      if (this.cancelled) return;
      this.state.advance(Math.max(0, now - this.lastTick) / 1000);
      this.lastTick = now;
      this.draw();
      this.progress(!this.state.playing);
      this.schedule();
    });
  }
  command(command) {
    if (!this.ready || this.cancelled) return;
    switch (command.type) {
      case 'play': this.state.play(); this.lastTick = performance.now(); break;
      case 'pause': this.state.pause(); break;
      case 'seek': this.state.seek(command.distance); break;
      case 'speed':
        if (Number.isFinite(command.value) && command.value >= 0.25 && command.value <= 4) this.state.speed = command.value;
        break;
      case 'restoreCamera':
      case 'camera': {
        const offsets = { ...this.state.offsets };
        if (command.type === 'restoreCamera') {
          if (!this.state.restoreCamera(command.snapshot)) return;
        } else this.state.camera(command.mode);
        this.reset = command.type === 'camera' && command.mode === 'auto' && !this.reducedMotion
          ? { offsets, started: performance.now() } : null;
        if (this.state.mode === 'overview') {
          this.viewer.camera.lookAtTransform(C.Matrix4.IDENTITY);
          this.viewer.camera.viewBoundingSphere(this.bounds, new C.HeadingPitchRange(0, -0.85, Math.max(1000, this.bounds.radius * 3)));
          this.viewer.camera.lookAtTransform(C.Matrix4.IDENTITY);
        }
        break;
      }
      default: return;
    }
    this.draw();
    this.progress(true);
    this.schedule();
  }
  installGestures() {
    const canvas = this.viewer.canvas, pointers = new Map();
    const measure = () => {
      const points = [...pointers.values()];
      return { x: points.reduce((sum, point) => sum + point.x, 0) / points.length,
        y: points.reduce((sum, point) => sum + point.y, 0) / points.length,
        span: points.length > 1 ? Math.hypot(points[0].x - points[1].x, points[0].y - points[1].y) : 0 };
    };
    let previous;
    const down = event => {
      if (!this.ready) return;
      pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
      canvas.setPointerCapture(event.pointerId);
      previous = measure();
      event.preventDefault();
    };
    const move = event => {
      if (!pointers.has(event.pointerId)) return;
      pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
      const next = measure();
      this.state.adjust((previous.x - next.x) * 0.006, (next.y - previous.y) * 0.004,
        previous.span > 0 && next.span > 0 ? previous.span / next.span : 1);
      previous = next;
      this.reset = null;
      this.draw(); this.progress();
      event.preventDefault();
    };
    const up = event => { pointers.delete(event.pointerId); previous = pointers.size ? measure() : null; this.progress(true); };
    const wheel = event => {
      if (!this.ready) return;
      this.state.adjust(0, 0, Math.exp(clamp(event.deltaY, -100, 100) * 0.005));
      this.reset = null;
      this.draw(); this.progress(); event.preventDefault();
    };
    for (const [type, handler] of [['pointerdown', down], ['pointermove', move], ['pointerup', up], ['pointercancel', up], ['wheel', wheel]]) {
      canvas.addEventListener(type, handler, { passive: false });
      this.disposers.push(() => canvas.removeEventListener(type, handler));
    }
  }
  recordFrame() {
    const now = performance.now();
    if (this.state.playing && this.lastFrame) this.frameTimes.push(now - this.lastFrame);
    this.lastFrame = this.state.playing ? now : 0;
    if (now - this.metricsAt < 5000) return;
    const sorted = this.frameTimes.sort((a, b) => a - b);
    send({ type: 'metrics', fps: sorted.length ? 1000 * sorted.length / sorted.reduce((a, b) => a + b, 0) : 0,
      p95ms: sorted[Math.floor(sorted.length * 0.95)] ?? 0, pendingTiles: this.pendingTiles, tileErrors: this.tileErrors });
    this.frameTimes = []; this.metricsAt = now;
  }
  fail(message) {
    if (this.cancelled) return;
    this.destroy();
    send({ type: 'error', message });
  }
  destroy() {
    this.cancelled = true;
    this.state.pause();
    this.gapAnimation?.cancel();
    clearTimeout(this.timer); clearTimeout(this.progressTimer); clearInterval(this.loadPoll); cancelAnimationFrame(this.frame);
    for (const dispose of this.disposers) dispose();
    this.disposers = [];
    if (this.viewer && !this.viewer.isDestroyed()) this.viewer.destroy();
  }
}

window.obcReplay = command => {
  if (!command || typeof command !== 'object') return;
  try {
    if (command.type === 'load') {
      session?.destroy();
      session = new Replay(command);
      const current = session;
      current.load().catch(error => current.fail(String(error.message ?? error)));
    } else if (command.type === 'destroy') { session?.destroy(); session = null; }
    else session?.command(command);
  } catch (error) {
    session?.destroy();
    send({ type: 'error', message: String(error.message ?? error) });
  }
};
document.addEventListener('visibilitychange', () => {
  if (document.hidden) session?.command({ type: 'pause' });
});
window.addEventListener('pagehide', () => { session?.destroy(); session = null; });
window.addEventListener('unhandledrejection', event => session?.fail(String(event.reason?.message ?? event.reason)));
window.addEventListener('error', () => session?.fail('The map renderer stopped. Try again.'));
send({ type: 'booted' });
