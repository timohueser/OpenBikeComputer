/* Minimal interactive GLB viewer for `figure.model` blocks (the ```model directive).

   Hand-rolled on purpose: the .glb files come from our own STEP converter
   (docs/tools/step2glb.py), so we only need the subset CAD conversions produce —
   triangle primitives with POSITION / optional NORMAL / optional indices, node
   TRS/matrix transforms, and a baseColorFactor per material. That keeps the whole
   viewer a few KB instead of vendoring a full engine.

   Interactions: drag = orbit · wheel / pinch = zoom · shift/right-drag or
   two-finger drag = pan · double-click = reset. Slow auto-orbit until first touch. */
(function () {
  "use strict";

  /* ----------------------------------------------------------------- mat math */
  function perspective(fovy, aspect, near, far) {
    var f = 1 / Math.tan(fovy / 2), nf = 1 / (near - far);
    return [f / aspect, 0, 0, 0, 0, f, 0, 0, 0, 0, (far + near) * nf, -1,
            0, 0, 2 * far * near * nf, 0];
  }
  function lookAt(eye, c, up) {
    var z = norm3(sub3(eye, c)), x = norm3(cross3(up, z)), y = cross3(z, x);
    return [x[0], y[0], z[0], 0, x[1], y[1], z[1], 0, x[2], y[2], z[2], 0,
            -dot3(x, eye), -dot3(y, eye), -dot3(z, eye), 1];
  }
  function mul4(a, b) {
    var o = new Array(16);
    for (var c = 0; c < 4; c++) for (var r = 0; r < 4; r++) {
      o[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] +
                     a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
    }
    return o;
  }
  function sub3(a, b) { return [a[0] - b[0], a[1] - b[1], a[2] - b[2]]; }
  function cross3(a, b) {
    return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  }
  function dot3(a, b) { return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]; }
  function norm3(a) {
    var l = Math.sqrt(dot3(a, a)) || 1;
    return [a[0] / l, a[1] / l, a[2] / l];
  }
  function trsMatrix(node) {
    if (node.matrix) return node.matrix;
    var t = node.translation || [0, 0, 0];
    var q = node.rotation || [0, 0, 0, 1];
    var s = node.scale || [1, 1, 1];
    var x = q[0], y = q[1], z = q[2], w = q[3];
    var m = [
      (1 - 2 * (y * y + z * z)) * s[0], (2 * (x * y + z * w)) * s[0], (2 * (x * z - y * w)) * s[0], 0,
      (2 * (x * y - z * w)) * s[1], (1 - 2 * (x * x + z * z)) * s[1], (2 * (y * z + x * w)) * s[1], 0,
      (2 * (x * z + y * w)) * s[2], (2 * (y * z - x * w)) * s[2], (1 - 2 * (x * x + y * y)) * s[2], 0,
      t[0], t[1], t[2], 1];
    return m;
  }
  function xformPoint(m, p, i, out, o) {
    var x = p[i], y = p[i + 1], z = p[i + 2];
    out[o] = m[0] * x + m[4] * y + m[8] * z + m[12];
    out[o + 1] = m[1] * x + m[5] * y + m[9] * z + m[13];
    out[o + 2] = m[2] * x + m[6] * y + m[10] * z + m[14];
  }
  function xformDir(m, p, i, out, o) {
    var x = p[i], y = p[i + 1], z = p[i + 2];
    out[o] = m[0] * x + m[4] * y + m[8] * z;
    out[o + 1] = m[1] * x + m[5] * y + m[9] * z;
    out[o + 2] = m[2] * x + m[6] * y + m[10] * z;
  }

  /* ---------------------------------------------------------------- GLB parse */
  var CTOR = { 5120: Int8Array, 5121: Uint8Array, 5122: Int16Array,
               5123: Uint16Array, 5125: Uint32Array, 5126: Float32Array };
  var NCOMP = { SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4, MAT4: 16 };

  function parseGLB(buf) {
    var dv = new DataView(buf);
    if (dv.getUint32(0, true) !== 0x46546c67) throw new Error("not a GLB file");
    var json = null, bin = null, off = 12;
    while (off < buf.byteLength) {
      var len = dv.getUint32(off, true), type = dv.getUint32(off + 4, true);
      var chunk = buf.slice(off + 8, off + 8 + len);
      if (type === 0x4e4f534a) json = JSON.parse(new TextDecoder().decode(chunk));
      else if (type === 0x004e4942) bin = chunk;
      off += 8 + len + (len % 4 ? 4 - (len % 4) : 0);
    }
    if (!json || !bin) throw new Error("GLB missing JSON or BIN chunk");
    return { json: json, bin: bin };
  }

  function accessor(glb, idx) {
    var acc = glb.json.accessors[idx];
    var bv = glb.json.bufferViews[acc.bufferView];
    var Ctor = CTOR[acc.componentType];
    var ncomp = NCOMP[acc.type];
    var base = (bv.byteOffset || 0) + (acc.byteOffset || 0);
    var packed = ncomp * Ctor.BYTES_PER_ELEMENT;
    if (bv.byteStride && bv.byteStride !== packed) {
      var out = new Ctor(acc.count * ncomp);
      for (var i = 0; i < acc.count; i++) {
        var src = new Ctor(glb.bin, base + i * bv.byteStride, ncomp);
        out.set(src, i * ncomp);
      }
      return out;
    }
    return new Ctor(glb.bin.slice(base, base + acc.count * packed));
  }

  function flatNormals(pos) {
    var nrm = new Float32Array(pos.length);
    for (var i = 0; i < pos.length; i += 9) {
      var ux = pos[i + 3] - pos[i], uy = pos[i + 4] - pos[i + 1], uz = pos[i + 5] - pos[i + 2];
      var vx = pos[i + 6] - pos[i], vy = pos[i + 7] - pos[i + 1], vz = pos[i + 8] - pos[i + 2];
      var n = norm3([uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx]);
      for (var k = 0; k < 3; k++) {
        nrm[i + k * 3] = n[0]; nrm[i + k * 3 + 1] = n[1]; nrm[i + k * 3 + 2] = n[2];
      }
    }
    return nrm;
  }

  /* Walk the scene graph, bake node transforms into world-space primitives. */
  function extractPrims(glb) {
    var g = glb.json, prims = [];
    function visit(nodeIdx, parent) {
      var node = g.nodes[nodeIdx];
      var world = mul4(parent, trsMatrix(node));
      if (node.mesh !== undefined) {
        g.meshes[node.mesh].primitives.forEach(function (p) {
          if (p.mode !== undefined && p.mode !== 4) return;   // triangles only
          var posA = accessor(glb, p.attributes.POSITION);
          var nrmA = p.attributes.NORMAL !== undefined ? accessor(glb, p.attributes.NORMAL) : null;
          var idx = p.indices !== undefined ? accessor(glb, p.indices) : null;
          var pos = new Float32Array(posA.length);
          for (var i = 0; i < posA.length; i += 3) xformPoint(world, posA, i, pos, i);
          var nrm = null;
          if (nrmA) {
            nrm = new Float32Array(nrmA.length);
            for (var j = 0; j < nrmA.length; j += 3) xformDir(world, nrmA, j, nrm, j);
          } else {
            // No normals: de-index and take flat face normals.
            var flat = new Float32Array((idx ? idx.length : pos.length / 3) * 3);
            if (idx) {
              for (var t = 0; t < idx.length; t++) {
                flat[t * 3] = pos[idx[t] * 3];
                flat[t * 3 + 1] = pos[idx[t] * 3 + 1];
                flat[t * 3 + 2] = pos[idx[t] * 3 + 2];
              }
              idx = null;
            } else {
              flat = pos;
            }
            pos = flat;
            nrm = flatNormals(pos);
          }
          var color = [0.69, 0.71, 0.63];
          if (p.material !== undefined) {
            var mat = g.materials[p.material];
            var pbr = mat && mat.pbrMetallicRoughness;
            if (pbr && pbr.baseColorFactor) color = pbr.baseColorFactor.slice(0, 3);
          }
          prims.push({ pos: pos, nrm: nrm, idx: idx, color: color });
        });
      }
      (node.children || []).forEach(function (c) { visit(c, world); });
    }
    var I = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
    var scene = g.scenes[g.scene || 0];
    scene.nodes.forEach(function (n) { visit(n, I); });
    return prims;
  }

  /* ------------------------------------------------------------------ shaders */
  var VS =
    "attribute vec3 aPos; attribute vec3 aNrm;" +
    "uniform mat4 uMVP; varying vec3 vN; varying vec3 vP;" +
    "void main(){ vN = aNrm; vP = aPos; gl_Position = uMVP * vec4(aPos, 1.0); }";
  var FS =
    "precision mediump float;" +
    "uniform vec3 uEye; uniform vec3 uColor;" +
    "varying vec3 vN; varying vec3 vP;" +
    "void main(){" +
    "  vec3 V = normalize(uEye - vP);" +
    "  vec3 N = normalize(vN);" +
    "  if (dot(N, V) < 0.0) N = -N;" +                 /* double-sided, CAD-safe */
    "  float d1 = max(dot(N, normalize(vec3(0.5, 0.85, 0.55))), 0.0);" +
    "  float d2 = max(dot(N, normalize(vec3(-0.65, 0.25, -0.5))), 0.0);" +
    "  float head = max(dot(N, V), 0.0);" +
    "  vec3 c = uColor * (0.28 + 0.52 * d1 + 0.22 * d2 + 0.16 * head);" +
    "  c += vec3(0.93, 0.89, 0.72) * pow(1.0 - head, 3.0) * 0.18;" +  /* parchment rim */
    "  gl_FragColor = vec4(pow(c, vec3(1.0 / 1.7)), 1.0);" +
    "}";

  function compile(gl, type, src) {
    var s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
      throw new Error("shader: " + gl.getShaderInfoLog(s));
    }
    return s;
  }

  /* ------------------------------------------------------------------- viewer */
  function startViewer(stage, glbBuf) {
    var glb = parseGLB(glbBuf);
    var prims = extractPrims(glb);
    if (!prims.length) throw new Error("no triangle meshes in GLB");

    // Bounds -> center + framing distance.
    var mn = [1e30, 1e30, 1e30], mx = [-1e30, -1e30, -1e30];
    prims.forEach(function (p) {
      for (var i = 0; i < p.pos.length; i += 3) {
        for (var k = 0; k < 3; k++) {
          if (p.pos[i + k] < mn[k]) mn[k] = p.pos[i + k];
          if (p.pos[i + k] > mx[k]) mx[k] = p.pos[i + k];
        }
      }
    });
    var center0 = [(mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2, (mn[2] + mx[2]) / 2];
    var radius = Math.max(0.5 * Math.sqrt(
      (mx[0] - mn[0]) * (mx[0] - mn[0]) + (mx[1] - mn[1]) * (mx[1] - mn[1]) +
      (mx[2] - mn[2]) * (mx[2] - mn[2])), 1e-6);
    var FOV = 40 * Math.PI / 180;
    var dist0 = radius / Math.tan(FOV / 2) * 1.35;

    var canvas = document.createElement("canvas");
    stage.appendChild(canvas);
    var gl = canvas.getContext("webgl", { antialias: true, alpha: true });
    if (!gl) throw new Error("WebGL unavailable");
    var uintExt = gl.getExtension("OES_element_index_uint");

    var prog = gl.createProgram();
    gl.attachShader(prog, compile(gl, gl.VERTEX_SHADER, VS));
    gl.attachShader(prog, compile(gl, gl.FRAGMENT_SHADER, FS));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
      throw new Error("link: " + gl.getProgramInfoLog(prog));
    }
    gl.useProgram(prog);
    var loc = {
      aPos: gl.getAttribLocation(prog, "aPos"),
      aNrm: gl.getAttribLocation(prog, "aNrm"),
      uMVP: gl.getUniformLocation(prog, "uMVP"),
      uEye: gl.getUniformLocation(prog, "uEye"),
      uColor: gl.getUniformLocation(prog, "uColor"),
    };

    prims.forEach(function (p) {
      if (p.idx instanceof Uint32Array && !uintExt) {
        // No 32-bit index support: de-index into flat triangles.
        var flat = new Float32Array(p.idx.length * 3), fn = new Float32Array(p.idx.length * 3);
        for (var t = 0; t < p.idx.length; t++) {
          for (var k = 0; k < 3; k++) {
            flat[t * 3 + k] = p.pos[p.idx[t] * 3 + k];
            fn[t * 3 + k] = p.nrm[p.idx[t] * 3 + k];
          }
        }
        p.pos = flat; p.nrm = fn; p.idx = null;
      }
      p.posBuf = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, p.posBuf);
      gl.bufferData(gl.ARRAY_BUFFER, p.pos, gl.STATIC_DRAW);
      p.nrmBuf = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, p.nrmBuf);
      gl.bufferData(gl.ARRAY_BUFFER, p.nrm, gl.STATIC_DRAW);
      p.count = p.pos.length / 3;
      if (p.idx) {
        p.idxBuf = gl.createBuffer();
        gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, p.idxBuf);
        gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, p.idx, gl.STATIC_DRAW);
        p.idxType = p.idx instanceof Uint32Array ? gl.UNSIGNED_INT : gl.UNSIGNED_SHORT;
        p.count = p.idx.length;
      }
      p.pos = p.nrm = p.idx = null;   // uploaded; let GC take the CPU copies
    });

    // Orbit state.
    var theta = 0.7, phi = 0.42, dist = dist0, target = center0.slice();
    var autoSpin = true, needsDraw = true;
    // rAF discipline: the loop only re-arms itself while auto-spinning in view.
    // Once the user grabs the model (autoSpin off), each interaction schedules a
    // single frame via invalidate() and the viewer costs nothing while idle.
    var inView = true, rafPending = false;

    function schedule() {
      if (!rafPending) { rafPending = true; requestAnimationFrame(frame); }
    }
    function invalidate() {
      needsDraw = true;
      if (inView) schedule();
    }

    function eyePos() {
      return [
        target[0] + dist * Math.cos(phi) * Math.sin(theta),
        target[1] + dist * Math.sin(phi),
        target[2] + dist * Math.cos(phi) * Math.cos(theta)];
    }

    function resize() {
      var dpr = Math.min(window.devicePixelRatio || 1, 2);
      var w = Math.round(stage.clientWidth * dpr), h = Math.round(stage.clientHeight * dpr);
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w; canvas.height = h;
        gl.viewport(0, 0, w, h);
        needsDraw = true;
      }
    }

    function draw() {
      resize();
      gl.clearColor(0, 0, 0, 0);
      gl.enable(gl.DEPTH_TEST);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      var eye = eyePos();
      var proj = perspective(FOV, canvas.width / Math.max(canvas.height, 1),
                             dist / 100, dist * 10 + radius * 4);
      var mvp = mul4(proj, lookAt(eye, target, [0, 1, 0]));
      gl.uniformMatrix4fv(loc.uMVP, false, new Float32Array(mvp));
      gl.uniform3fv(loc.uEye, eye);
      prims.forEach(function (p) {
        gl.bindBuffer(gl.ARRAY_BUFFER, p.posBuf);
        gl.enableVertexAttribArray(loc.aPos);
        gl.vertexAttribPointer(loc.aPos, 3, gl.FLOAT, false, 0, 0);
        gl.bindBuffer(gl.ARRAY_BUFFER, p.nrmBuf);
        gl.enableVertexAttribArray(loc.aNrm);
        gl.vertexAttribPointer(loc.aNrm, 3, gl.FLOAT, false, 0, 0);
        gl.uniform3fv(loc.uColor, p.color);
        if (p.idxBuf) {
          gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, p.idxBuf);
          gl.drawElements(gl.TRIANGLES, p.count, p.idxType, 0);
        } else {
          gl.drawArrays(gl.TRIANGLES, 0, p.count);
        }
      });
    }

    function frame() {
      rafPending = false;
      if (autoSpin) { theta += 0.0035; needsDraw = true; }
      if (needsDraw) { draw(); needsDraw = false; }
      if (inView && autoSpin) schedule();
    }
    schedule();

    // Pause the auto-spin (and any drawing) while scrolled out of view.
    if ("IntersectionObserver" in window) {
      new IntersectionObserver(function (entries) {
        inView = entries.some(function (e) { return e.isIntersecting; });
        if (inView) schedule();
      }).observe(stage);
    }

    /* ------------------------------------------------------------ interaction */
    function stopSpin() { autoSpin = false; }
    function panBy(dx, dy) {
      // Screen-space pan mapped to the camera's right/up axes.
      var eye = eyePos();
      var fwd = norm3(sub3(target, eye));
      var right = norm3(cross3(fwd, [0, 1, 0]));
      var up = cross3(right, fwd);
      var k = dist * Math.tan(FOV / 2) * 2 / stage.clientHeight;
      for (var i = 0; i < 3; i++) target[i] += (-dx * right[i] + dy * up[i]) * k;
    }

    var pointers = {}, lastPinch = 0;
    stage.addEventListener("pointerdown", function (e) {
      stopSpin();
      stage.focus({ preventScroll: true });   // engages wheel-zoom (see wheel handler)
      stage.classList.add("dragging");
      stage.setPointerCapture(e.pointerId);
      pointers[e.pointerId] = { x: e.clientX, y: e.clientY, btn: e.button, shift: e.shiftKey };
      if (Object.keys(pointers).length === 2) {
        var ids = Object.keys(pointers);
        lastPinch = Math.hypot(pointers[ids[0]].x - pointers[ids[1]].x,
                               pointers[ids[0]].y - pointers[ids[1]].y);
      }
      e.preventDefault();
    });
    stage.addEventListener("pointermove", function (e) {
      var p = pointers[e.pointerId];
      if (!p) return;
      var dx = e.clientX - p.x, dy = e.clientY - p.y;
      p.x = e.clientX; p.y = e.clientY;
      var ids = Object.keys(pointers);
      if (ids.length === 2) {
        // Pinch zoom + two-finger pan.
        var a = pointers[ids[0]], b = pointers[ids[1]];
        var pinch = Math.hypot(a.x - b.x, a.y - b.y);
        if (lastPinch) dist = Math.min(dist0 * 6, Math.max(dist0 * 0.15, dist * lastPinch / pinch));
        lastPinch = pinch;
        panBy(dx / 2, dy / 2);
      } else if (p.btn === 2 || p.btn === 1 || p.shift) {
        panBy(dx, dy);
      } else {
        theta -= dx * 0.008;
        phi = Math.min(1.5, Math.max(-1.5, phi + dy * 0.008));
      }
      invalidate();
    });
    function release(e) {
      delete pointers[e.pointerId];
      lastPinch = 0;
      if (!Object.keys(pointers).length) stage.classList.remove("dragging");
    }
    stage.addEventListener("pointerup", release);
    stage.addEventListener("pointercancel", release);
    stage.addEventListener("contextmenu", function (e) { e.preventDefault(); });
    stage.addEventListener("wheel", function (e) {
      // Zoom only once the viewer is engaged (clicked or tabbed to) or with a
      // modifier held — a bare scroll over the full-width stage must keep
      // scrolling the page, not hijack it into a zoom.
      if (document.activeElement !== stage && !e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      stopSpin();
      dist = Math.min(dist0 * 6, Math.max(dist0 * 0.15, dist * Math.pow(1.1, e.deltaY > 0 ? 1 : -1)));
      invalidate();
    }, { passive: false });
    stage.addEventListener("dblclick", function (e) {
      e.preventDefault();
      theta = 0.7; phi = 0.42; dist = dist0; target = center0.slice();
      invalidate();
    });
    stage.addEventListener("keydown", function (e) {
      var step = 0.12;
      if (e.key === "ArrowLeft") theta -= step;
      else if (e.key === "ArrowRight") theta += step;
      else if (e.key === "ArrowUp") phi = Math.min(1.5, phi + step);
      else if (e.key === "ArrowDown") phi = Math.max(-1.5, phi - step);
      else if (e.key === "+" || e.key === "=") dist = Math.max(dist0 * 0.15, dist / 1.15);
      else if (e.key === "-") dist = Math.min(dist0 * 6, dist * 1.15);
      else return;
      stopSpin();
      invalidate();
      e.preventDefault();
    });

    if ("ResizeObserver" in window) new ResizeObserver(invalidate).observe(stage);
  }

  /* -------------------------------------------------------------------- init */
  function message(stage, text) {
    var msg = document.createElement("div");
    msg.className = "model-msg";
    msg.textContent = text;
    stage.appendChild(msg);
  }

  function boot(fig) {
    var stage = fig.querySelector(".model-stage");
    var src = fig.getAttribute("data-glb");
    if (!stage || !src) return;
    var loaded = false;
    function load() {
      if (loaded) return;
      loaded = true;
      message(stage, "loading model…");
      fetch(src).then(function (r) {
        if (!r.ok) throw new Error("HTTP " + r.status);
        return r.arrayBuffer();
      }).then(function (buf) {
        stage.textContent = "";
        startViewer(stage, buf);
      }).catch(function (err) {
        stage.textContent = "";
        message(stage, "couldn't load model (" + err.message + ")");
      });
    }
    if ("IntersectionObserver" in window) {
      var io = new IntersectionObserver(function (entries) {
        if (entries.some(function (e) { return e.isIntersecting; })) {
          io.disconnect();
          load();
        }
      }, { rootMargin: "200px" });
      io.observe(stage);
    } else {
      load();
    }
  }

  function init() {
    Array.prototype.forEach.call(document.querySelectorAll("figure.model"), boot);
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
