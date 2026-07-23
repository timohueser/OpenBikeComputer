/* Blog enhancements: image lightbox (zoom/pan) + before/after compare slider.
   Plain ES5-ish, no dependencies; everything degrades gracefully without JS. */
(function () {
  "use strict";

  /* ------------------------------------------------------------------ lightbox */
  function buildLightbox() {
    var lb = document.createElement("div");
    lb.className = "lb";
    lb.setAttribute("role", "dialog");
    lb.setAttribute("aria-modal", "true");
    lb.innerHTML =
      '<div class="lb-hint">scroll to zoom · drag to pan · esc to close</div>' +
      '<img alt="">' +
      '<div class="lb-cap"></div>' +
      '<button class="lb-close" aria-label="Close">×</button>';
    document.body.appendChild(lb);

    var img = lb.querySelector("img");
    var cap = lb.querySelector(".lb-cap");
    var closeBtn = lb.querySelector(".lb-close");
    var scale = 1, tx = 0, ty = 0;
    var opener = null;   // element to hand focus back to on close

    function apply() {
      img.style.transform = "translate(" + tx + "px," + ty + "px) scale(" + scale + ")";
    }
    function reset() { scale = 1; tx = 0; ty = 0; apply(); }
    function close() {
      lb.classList.remove("open");
      document.body.style.overflow = "";
      if (opener && opener.focus) opener.focus();
      opener = null;
    }

    lb.open = function (src, caption) {
      opener = document.activeElement;
      img.src = src;
      cap.textContent = caption || "";
      reset();
      lb.classList.add("open");
      document.body.style.overflow = "hidden";
      closeBtn.focus();   // put keyboard users inside the dialog
    };

    lb.addEventListener("click", function (e) {
      if (e.target !== img) close();
    });
    document.addEventListener("keydown", function (e) {
      if (e.key === "Escape" && lb.classList.contains("open")) close();
    });

    lb.addEventListener("wheel", function (e) {
      if (!lb.classList.contains("open")) return;
      e.preventDefault();
      var factor = e.deltaY < 0 ? 1.18 : 1 / 1.18;
      var next = Math.min(8, Math.max(1, scale * factor));
      // Zoom around the cursor: keep the point under the pointer fixed.
      var r = img.getBoundingClientRect();
      var cx = e.clientX - (r.left + r.width / 2);
      var cy = e.clientY - (r.top + r.height / 2);
      tx -= cx * (next / scale - 1);
      ty -= cy * (next / scale - 1);
      scale = next;
      if (scale === 1) { tx = 0; ty = 0; }
      apply();
    }, { passive: false });

    img.addEventListener("dblclick", function (e) {
      e.preventDefault();
      if (scale > 1) { reset(); } else { scale = 2.5; apply(); }
    });

    // One pointer pans (only once zoomed — at scale 1 there is nothing to pan);
    // two pointers pinch-zoom, so touch users aren't stuck with double-tap only.
    var pointers = {}, lastPinch = 0;
    img.addEventListener("pointerdown", function (e) {
      e.preventDefault();
      pointers[e.pointerId] = { x: e.clientX, y: e.clientY };
      img.setPointerCapture(e.pointerId);
      var ids = Object.keys(pointers);
      if (ids.length === 2) {
        lastPinch = Math.hypot(pointers[ids[0]].x - pointers[ids[1]].x,
                               pointers[ids[0]].y - pointers[ids[1]].y);
      } else if (scale > 1) {
        img.classList.add("panning");
      }
    });
    img.addEventListener("pointermove", function (e) {
      var p = pointers[e.pointerId];
      if (!p) return;
      var dx = e.clientX - p.x, dy = e.clientY - p.y;
      p.x = e.clientX; p.y = e.clientY;
      var ids = Object.keys(pointers);
      if (ids.length === 2) {
        var a = pointers[ids[0]], b = pointers[ids[1]];
        var pinch = Math.hypot(a.x - b.x, a.y - b.y);
        if (lastPinch) {
          scale = Math.min(8, Math.max(1, scale * pinch / lastPinch));
          if (scale === 1) { tx = 0; ty = 0; }
        }
        lastPinch = pinch;
        tx += dx / 2; ty += dy / 2;
        apply();
      } else if (scale > 1) {
        tx += dx; ty += dy;
        apply();
      }
    });
    function release(e) {
      delete pointers[e.pointerId];
      lastPinch = 0;
      if (!Object.keys(pointers).length) img.classList.remove("panning");
    }
    img.addEventListener("pointerup", release);
    img.addEventListener("pointercancel", release);
    return lb;
  }

  var lightbox = null;
  function initLightbox() {
    var imgs = document.querySelectorAll(".post .prose img, .prose.post img");
    Array.prototype.forEach.call(imgs, function (im) {
      if (im.closest(".cmp") || im.closest(".model") || im.closest(".lb")) return;
      im.addEventListener("click", function () {
        if (!lightbox) lightbox = buildLightbox();
        lightbox.open(im.currentSrc || im.src, im.getAttribute("alt"));
      });
    });
  }

  /* ------------------------------------------------------------ compare slider */
  function initCompare(fig) {
    var stage = fig.querySelector(".cmp-stage");
    if (!stage || stage.querySelectorAll("img").length < 2) return;

    var handle = document.createElement("div");
    handle.className = "cmp-handle";
    stage.appendChild(handle);
    ["before", "after"].forEach(function (side) {
      var tag = document.createElement("span");
      tag.className = "cmp-tag " + side;
      tag.textContent = fig.getAttribute("data-label-" + side) || side;
      stage.appendChild(tag);
    });

    var pct = 50;
    function set(p) {
      pct = Math.min(100, Math.max(0, p));
      stage.style.setProperty("--cmp", pct + "%");
      stage.setAttribute("aria-valuenow", Math.round(pct));
    }
    stage.setAttribute("role", "slider");
    stage.setAttribute("aria-label", "Image comparison");
    stage.setAttribute("aria-valuemin", "0");
    stage.setAttribute("aria-valuemax", "100");
    stage.setAttribute("tabindex", "0");
    set(50);

    function fromEvent(e) {
      var r = stage.getBoundingClientRect();
      set(((e.clientX - r.left) / r.width) * 100);
    }
    var down = false;
    stage.addEventListener("pointerdown", function (e) {
      down = true;
      stage.setPointerCapture(e.pointerId);
      fromEvent(e);
    });
    stage.addEventListener("pointermove", function (e) { if (down) fromEvent(e); });
    stage.addEventListener("pointerup", function () { down = false; });
    // touch-action: pan-y — the browser takes over vertical swipes and cancels us.
    stage.addEventListener("pointercancel", function () { down = false; });
    stage.addEventListener("keydown", function (e) {
      if (e.key === "ArrowLeft") { set(pct - 4); e.preventDefault(); }
      if (e.key === "ArrowRight") { set(pct + 4); e.preventDefault(); }
    });
  }

  function init() {
    initLightbox();
    Array.prototype.forEach.call(document.querySelectorAll("figure.cmp"), initCompare);
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
