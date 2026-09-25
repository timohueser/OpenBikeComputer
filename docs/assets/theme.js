(function () {
  var system = matchMedia('(prefers-color-scheme: dark)');
  var preference;
  try { preference = localStorage.getItem('obc-theme'); } catch (_) {}
  function apply() {
    var dark = preference === 'dark' || (preference !== 'light' && system.matches);
    document.documentElement.dataset.theme = dark ? 'dark' : 'light';
    document.querySelector('meta[name="theme-color"]').content = dark ? '#93461A' : '#A4501E';
    var button = document.getElementById('theme_toggle');
    if (button) {
      button.hidden = false;
      button.setAttribute('aria-pressed', String(dark));
      button.title = dark ? 'Switch to light mode' : 'Switch to dark mode';
    }
    dispatchEvent(new Event('obc-theme-change'));
  }
  apply(); // Resolve before first paint, including a saved override.
  system.addEventListener('change', apply);
  addEventListener('storage', function (event) {
    if (event.key !== 'obc-theme' && event.key !== null) return;
    try { preference = localStorage.getItem('obc-theme'); } catch (_) {}
    apply();
  });
  document.addEventListener('DOMContentLoaded', function () {
    apply();
    document.getElementById('theme_toggle')?.addEventListener('click', function () {
      preference = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
      try { localStorage.setItem('obc-theme', preference); } catch (_) {}
      apply();
    });
  });
})();
