/* Scale each .fit's child to the available width. data-w = child width, data-max = largest zoom. */
function fitAll() {
  document.querySelectorAll('.fit').forEach(function (f) {
    var c = f.firstElementChild; if (!c) return;
    var w = +f.dataset.w || c.offsetWidth, max = +(f.dataset.max || 1);
    c.style.zoom = Math.min(max, f.clientWidth / w);
  });
}
addEventListener('resize', fitAll);
document.addEventListener('DOMContentLoaded', fitAll);
fitAll();
