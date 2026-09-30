(() => {
  const report = {requests: [], violations: [], resourceErrors: [], errors: []};
  const originalFetch = fetch;
  globalThis.fetch = async (...args) => {
    const url = new URL(args[0] instanceof Request ? args[0].url : args[0], location.href).href;
    const start = performance.now();
    try {
      const response = await originalFetch(...args);
      report.requests.push({url, status: response.status, ms: performance.now() - start});
      return response;
    } catch (error) {
      report.requests.push({url, error: String(error), ms: performance.now() - start});
      throw error;
    }
  };
  addEventListener('securitypolicyviolation', event => report.violations.push({url: event.blockedURI, directive: event.effectiveDirective}));
  addEventListener('error', event => {
    if (event.target === window) report.errors.push(event.message);
    else report.resourceErrors.push(event.target?.src ?? event.target?.href ?? event.target?.tagName);
  }, true);
  addEventListener('unhandledrejection', event => report.errors.push(String(event.reason)));
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  async function wait(predicate, message, timeout = 60000) {
    const deadline = performance.now() + timeout;
    while (performance.now() < deadline) {
      const result = predicate();
      if (result) return result;
      await sleep(100);
    }
    throw Error(message + ': ' + document.body.innerText.slice(0, 1500));
  }
  const input = (element, text) => { element.value = text; element.dispatchEvent(new Event('input', {bubbles: true})); };
  const button = text => [...document.querySelectorAll('button')].find(b => b.textContent.trim() === text);
  const draftKey = 'obc-planner-routing-v2', versionsKey = 'obc-planner-lab-versions-v1';
  const snapshot = () => ({draft: localStorage.getItem(draftKey), versions: localStorage.getItem(versionsKey)});
  globalThis.runPlannerHostBenchmark = async (phase, previous, fullBW = false) => {
    const start = performance.now();
    const summaries = [];
    await wait(() => document.querySelector('input[aria-label="Find a place or ask about the route"]') && document.querySelector('canvas.maplibregl-canvas'), 'UI did not mount');
    report.phase = phase;
    if (phase === 'restore') {
      report.restored = snapshot();
      if (!previous || previous.draft !== report.restored.draft || previous.versions !== report.restored.versions)
        throw Error('The current draft or saved versions changed across process restart');
      if (!JSON.parse(report.restored.draft).points?.length) throw Error('Restored draft has no points');
    } else {
      input(document.querySelector('input[aria-label="Find a place or ask about the route"]'), 'route from Freiburg Hauptbahnhof to Augustinerplatz');
      const apply = await wait(() => document.querySelector('.results:not([aria-busy="true"]) .apply'), 'Smart route proposal did not resolve');
      apply.click();
      await wait(() => {
        const draft = JSON.parse(localStorage.getItem(draftKey) ?? 'null');
        return draft?.points?.some(p => p.kind === 'finish') && draft.points.some(p => p.kind === 'start');
      }, 'Applying the route did not save a draft');
      button('Save').click();
      const name = await wait(() => document.querySelector('input[aria-label="Version name"]'), 'Save did not open');
      input(name, 'Offline host benchmark');
      name.dispatchEvent(new Event('blur'));
      await wait(() => localStorage.getItem(versionsKey)?.includes('Offline host benchmark'), 'Saved version absent');
      document.querySelector('[aria-label="Saved versions"]')?.click();
    }
    const region = globalThis.__OBC_PLANNER_CONFIG__.region;
    async function request(path, body) {
      const start = performance.now();
      const response = await fetch(path, body ? {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body)} : undefined);
      const result = await response.json();
      if (!response.ok) throw Error(`${path}: ${response.status} ${JSON.stringify(result)}`);
      summaries.push({path, ms: performance.now() - start, type: result.type, results: result.results?.length,
        routes: result.routes?.length, geometry: result.routes?.[0]?.geometry?.length, features: result.features?.length,
        nullElevationSamples: result.routes?.[0]?.elevation?.filter(v => v === null).length,
        totals: result.routes?.[0]?.totals});
      return result;
    }
    const status = await request('/api/planner-search/status');
    if (!status.parser.ready || status.regions[0].id !== region) throw Error('Wrong local search region');
    const metadata = await request('/routing/v1/region');
    report.profiles = metadata.profiles;
    const points = [[7.8423399,47.9982505], [7.8521469,47.9937534]];
    report.unavailableGoals = [];
    for (const goal of ['least_unpaved','most_climbing']) {
      const response = await fetch('/api/planner-search/route', {method: 'POST', headers: {'Content-Type':'application/json'},
        body: JSON.stringify({points, bike: 'touring', goal})});
      const result = await response.json();
      if (response.ok || !result.error?.includes('routing package has no')) throw Error('Unsupported goal did not fail explicitly');
      report.unavailableGoals.push({goal, status: response.status, error: result.error});
    }
    for (const profile of ['touring','road','hiking']) {
      const result = await request('/routing/v1/route', {points, profile, alternatives: true});
      if (!(result.routes?.[0]?.geometry?.length > 1)) throw Error('Incomplete local routing result');
      const route = result.routes[0];
      const context = {region, view: [7.8,47.97,7.9,48.04], here: points[0],
        plan: {coordinates: route.geometry, points: [], days: [{number: 1, from: 0, to: 5, rest: false}]},
        startDate: '2026-09-28', now: '2026-09-28T10:58:00Z', limit: 20, submitted: true};
      const places = await request('/api/planner-search/query', {...context, q: 'cafes along the route open now'});
      summaries.at(-1).request = places.request;
      summaries.at(-1).hours = places.results?.filter(p => p.hoursStatus).length;
      await request('/api/planner-search/reverse', {region, coordinate: points[0]});
      for (const label of ['Zoom in','Zoom out','Show whole route']) {
        document.querySelector(`button[aria-label="${label}"]`)?.click();
        await sleep(700);
      }
      if (fullBW) {
        for (const [name, target] of [['day',[8.239,48.473]],['long',[9.177,48.776]]]) {
          await request('/routing/v1/route', {points: [[7.849,47.997],target], profile, alternatives: false});
          summaries.at(-1).case = name;
          summaries.at(-1).profile = profile;
        }
      }
    }
    if (fullBW) {
      document.querySelector('button[aria-label="Map settings"]').click();
      for (const network of ['Hiking','Cycling']) {
        button(network).click();
        for (const direction of ['Zoom out','Zoom in']) {
          for (let step = 0; step < 6; step++) {
            document.querySelector(`button[aria-label="${direction}"]`).click();
            await sleep(800);
          }
        }
      }
      document.querySelector('button[aria-label="Close map settings"]').click();
      document.querySelector('button[aria-label="Show whole route"]').click();
    }
    let blocked = false;
    try { await fetch('https://example.com/offline-planner-probe'); } catch { blocked = true; }
    if (!blocked) throw Error('External resource fetch escaped local-only policy');
    await sleep(1000);
    report.saved = snapshot();
    report.summaries = summaries;
    report.externalProbeBlocked = blocked;
    report.resources = performance.getEntriesByType('resource').map(r => ({url: r.name, duration: r.duration, bytes: r.transferSize}));
    report.unexpectedExternalRequests = [...report.requests, ...report.resources].filter(r =>
      /^https?:/.test(r.url) && new URL(r.url).origin !== location.origin && !r.url.endsWith('/offline-planner-probe'));
    if (report.unexpectedExternalRequests.length) throw Error('Planner attempted an unexpected external resource');
    report.durationMs = performance.now() - start;
    report.canvas = {width: document.querySelector('canvas.maplibregl-canvas').width, height: document.querySelector('canvas.maplibregl-canvas').height};
    report.body = document.body.innerText.slice(0, 2000);
    return JSON.stringify(report);
  };
})();
