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
    var scale = 1, tx = 0, ty = 0;

    function apply() {
      img.style.transform = "translate(" + tx + "px," + ty + "px) scale(" + scale + ")";
    }
    function reset() { scale = 1; tx = 0; ty = 0; apply(); }
    function close() {
      lb.classList.remove("open");
      document.body.style.overflow = "";
    }

    lb.open = function (src, caption) {
      img.src = src;
      cap.textContent = caption || "";
      reset();
      lb.classList.add("open");
      document.body.style.overflow = "hidden";
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

    var pan = null;
    img.addEventListener("pointerdown", function (e) {
      e.preventDefault();
      pan = { x: e.clientX - tx, y: e.clientY - ty };
      img.classList.add("panning");
      img.setPointerCapture(e.pointerId);
    });
    img.addEventListener("pointermove", function (e) {
      if (!pan) return;
      tx = e.clientX - pan.x;
      ty = e.clientY - pan.y;
      apply();
    });
    img.addEventListener("pointerup", function () {
      pan = null;
      img.classList.remove("panning");
    });
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
