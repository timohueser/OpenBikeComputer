(function () {
  "use strict";
  const MAP = JSON.parse(document.getElementById("system-map").textContent);
  const $ = (sel, root) => (root || document).querySelector(sel);
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  const fmt = (n) => Number(n).toLocaleString("en-US");
  // Relative to the explorer page: build_docs.py renders each system doc under it.
  const DOC_URL = "{name}/";
  const docUrl = (name) => DOC_URL.replace("{name}", encodeURIComponent(name));
  const RULE_LABELS = {
    cycle: "Cycle",
    R1: "Crate shared with another system",
    R2: "Code in two places",
    R3: "Shell too large",
    R4: "Outside the tooling folder",
    ownership: "Ownership",
    dependency: "Undeclared dependency",
  };

  // ---- Model -------------------------------------------------------------
  const cycleOf = new Map();
  MAP.cycles.forEach((c, i) => c.forEach((n) => cycleOf.set(n, i)));
  const sameCycle = (a, b) => cycleOf.has(a) && cycleOf.get(a) === cycleOf.get(b);
  const edges = MAP.edges.map((e) => Object.assign({}, e, { cycle: !e.test && sameCycle(e.from, e.to) }));

  // Areas whose members are all shells sort last; the rest sort by name.
  const shellArea = (a) => MAP.systems.filter((s) => s.area === a).every((s) => s.kind === "shell") ? 1 : 0;
  const areas = [...new Set(MAP.systems.map((s) => s.area))].sort((a, b) => shellArea(a) - shellArea(b) || a.localeCompare(b));
  const areaIdx = new Map(areas.map((a, i) => [a, i]));
  const systems = MAP.systems.slice().sort((a, b) => areaIdx.get(a.area) - areaIdx.get(b.area) || a.name.localeCompare(b.name));
  const byName = new Map(systems.map((s) => [s.name, s]));
  const isProblem = (s) => s.violations > 0 || cycleOf.has(s.name) || edges.some((e) => e.undeclared && (e.from === s.name || e.to === s.name));

  const defaultName = (systems.find((s) => s.status === "approved") || systems[0]).name;
  const state = {
    selected: null,
    view: "focus",
    includeTest: true,
    problemsOnly: false,
    areas: new Set(areas),
    hover: null,
    trail: [],
  };

  const liveEdges = () => edges.filter((e) => state.includeTest || !e.test);
  const visible = (s) => state.areas.has(s.area) && (!state.problemsOnly || isProblem(s));
  const visibleSystems = () => systems.filter(visible);
  const kinds = (e) => [e.test && "test", e.undeclared && "undecl", e.cycle && "cycle"].filter(Boolean).join(" ");
  const marker = (e) => e.undeclared ? "ah-bad" : e.cycle ? "ah-warn" : "ah-edge";
  const describe = (e) => {
    const bits = [`<span class="name">${esc(e.from)}</span> uses <span class="name">${esc(e.to)}</span>`];
    if (e.test) bits.push("test only");
    if (e.undeclared) bits.push("undeclared");
    if (e.cycle) bits.push("in a cycle");
    return bits.join(", ");
  };
  const curve = (sx, sy, ex, ey) => {
    const dx = (ex - sx) / 2;
    return `M${sx.toFixed(1)},${sy.toFixed(1)} C${(sx + dx).toFixed(1)},${sy.toFixed(1)} ${(ex - dx).toFixed(1)},${ey.toFixed(1)} ${ex.toFixed(1)},${ey.toFixed(1)}`;
  };
  const sizeSvg = (svg, root) => {
    const w = root.clientWidth, h = root.clientHeight;
    svg.setAttribute("viewBox", `0 0 ${w} ${h}`);
    return root.getBoundingClientRect();
  };

  // ---- Selection ---------------------------------------------------------
  function select(name, opts) {
    if (!byName.has(name)) return;
    const o = opts || {};
    if (!o.keepTrail) {
      state.trail = state.trail.filter((n) => n !== name);
      state.trail.push(name);
      if (state.trail.length > 8) state.trail.shift();
    }
    state.selected = name;
    if (o.view) state.view = o.view;
    if (decodeURIComponent(location.hash.slice(1)) !== name) {
      try { history.replaceState(null, "", "#" + encodeURIComponent(name)); } catch (_) { location.hash = name; }
    }
    render();
  }
  window.addEventListener("hashchange", () => select(decodeURIComponent(location.hash.slice(1))));

  // ---- Controls ----------------------------------------------------------
  function initControls() {
    const undecl = edges.filter((e) => e.undeclared).length;
    $("#stats").innerHTML = `<b>${systems.length}</b> systems · <b>${edges.length}</b> edges · ` +
      `<b class="bad">${undecl}</b> undeclared · <b class="warn">${MAP.cycles.length}</b> cycles`;
    $("#names").innerHTML = systems.map((s) => `<option value="${esc(s.name)}">`).join("");
    $("#areas").insertAdjacentHTML("beforeend", areas.map((a) =>
      `<label><input type="checkbox" value="${esc(a)}" checked> ${esc(a)}</label>`).join(""));

    document.querySelectorAll(".tabs button").forEach((b) => b.addEventListener("click", () => {
      state.view = b.dataset.view;
      render();
    }));
    const q = $("#q");
    const best = (v) => {
      v = v.trim().toLowerCase();
      if (!v) return null;
      return (systems.find((s) => s.name === v) || systems.find((s) => s.name.startsWith(v)) || systems.find((s) => s.name.includes(v)) || {}).name;
    };
    q.addEventListener("input", () => { if (byName.has(q.value.trim())) select(q.value.trim()); });
    q.addEventListener("keydown", (ev) => {
      if (ev.key !== "Enter") return;
      const n = best(q.value);
      if (n) { q.value = n; select(n); }
    });
    $("#areas").addEventListener("change", (ev) => {
      ev.target.checked ? state.areas.add(ev.target.value) : state.areas.delete(ev.target.value);
      render();
    });
    $("#opt-test").addEventListener("change", (ev) => { state.includeTest = ev.target.checked; render(); });
    $("#opt-problems").addEventListener("change", (ev) => { state.problemsOnly = ev.target.checked; render(); });

    $("#trail").addEventListener("click", (ev) => {
      const b = ev.target.closest("button");
      if (!b || b.disabled) return;
      if (b.classList.contains("back")) {
        state.trail.pop();
        select(state.trail[state.trail.length - 1], { keepTrail: true });
      } else if (!b.classList.contains("cur")) {
        state.trail = state.trail.slice(0, state.trail.indexOf(b.dataset.name) + 1);
        select(b.dataset.name, { keepTrail: true });
      }
    });

    // One delegated click handler for every element that names a system.
    $(".views").addEventListener("click", (ev) => {
      const t = ev.target.closest("[data-name]");
      if (!t) return;
      // A click re-renders the overview, so a dblclick event would reach a detached chip; use the click count.
      // The first click can wrap the trail and move the chips, so the second one opens the chip of the first.
      const again = t.matches(".chip") && ev.detail > 1;
      const toFocus = t.matches(".rl, .cl") || again;
      select(again ? state.selected : t.dataset.name, toFocus ? { view: "focus" } : undefined);
    });

    const ro = new ResizeObserver(() => drawWires());
    ro.observe($(".views"));
  }

  function renderTrail() {
    const t = state.trail;
    let h = `<button class="back" ${t.length < 2 ? "disabled" : ""} title="Previous selection">Back</button>`;
    h += t.map((n, i) => (i ? '<span class="gt">›</span>' : "") +
      `<button class="crumb${i === t.length - 1 ? " cur" : ""}" data-name="${esc(n)}">${esc(n)}</button>`).join("");
    $("#trail").innerHTML = h;
  }

  function lineSwatch(cls, mk) {
    return `<svg width="34" height="10"><path class="wire ${cls}" d="M1,5 L31,5" marker-end="url(#${mk})"/></svg>`;
  }
  function renderLegend() {
    let items;
    if (state.view === "matrix") {
      items = [["", "declared"], ["test", "test only"], ["undecl", "undeclared"], ["undecl cycle", "in a cycle"]]
        .map(([c, l]) => `<span><i class="m ${c}"></i>${l}</span>`);
      items.push('<span><i class="pdot" style="margin:0"></i>has problems</span>');
    } else if (state.view === "overview") {
      items = [
        `<span>${lineSwatch("uses", "ah-uses")}uses</span>`,
        `<span>${lineSwatch("usedby", "ah-usedby")}used by</span>`,
        `<span>${lineSwatch("uses test", "ah-uses")}test only</span>`,
        '<span><i class="ring"></i>in a cycle</span>',
        '<span><i class="sw-vio">3</i>violations</span>',
      ];
    } else {
      items = [
        `<span>${lineSwatch("", "ah-edge")}declared</span>`,
        `<span>${lineSwatch("test", "ah-edge")}test only</span>`,
        `<span>${lineSwatch("undecl", "ah-bad")}undeclared</span>`,
        `<span>${lineSwatch("cycle", "ah-warn")}in a cycle</span>`,
        '<span><i class="pdot" style="margin:0"></i>has problems</span>',
      ];
    }
    $("#legend").innerHTML = items.join("");
  }

  // ---- Focus view --------------------------------------------------------
  let focusModel = null;

  function nbCard(s, e, side) {
    const tags = [
      e.test && '<span class="tag test">test only</span>',
      e.undeclared && '<span class="tag undecl">undeclared</span>',
      e.cycle && '<span class="tag cycle">cycle</span>',
    ].filter(Boolean).join("");
    const glyph = side === "uses"
      ? `<svg class="glyph" width="30" height="10"><path class="wire ${kinds(e)}" d="M29,5 L2,5" marker-end="url(#${marker(e)})"/></svg>`
      : `<svg class="glyph" width="30" height="10"><path class="wire ${kinds(e)}" d="M1,5 L28,5" marker-end="url(#${marker(e)})"/></svg>`;
    return `<button class="nb${isProblem(s) ? " problem" : ""}" data-name="${esc(s.name)}">${glyph}` +
      `<span class="nb-name">${esc(s.name)}</span>` +
      `<span class="nb-meta">${esc(s.area)} · ${fmt(s.lines)} lines</span><span class="tags">${tags}</span></button>`;
  }

  function column(side, title, items, hidden, emptyText) {
    const n = `<span class="n">${items.length}${hidden ? ` (${hidden} hidden)` : ""}</span>`;
    const body = items.length ? items.map((x) => nbCard(x.s, x.e, side)).join("") : `<div class="empty">${emptyText}</div>`;
    return `<section class="col ${side}"><h3>${title} ${n}</h3><div class="stack">${body}</div></section>`;
  }

  // "a -> b closes a cycle: x -> y, x -> z (test)"; null when the message has another shape.
  function parseCycle(msg) {
    const m = /^(\S+) -> (\S+) closes a cycle: (.+)$/.exec(msg);
    if (!m) return null;
    const pairs = m[3].split(", ").map((p) => /^(\S+) -> (\S+?)( \(test\))?$/.exec(p));
    if (pairs.some((p) => !p)) return null;
    return { from: m[1], to: m[2], pairs: pairs.map((p) => ({ from: p[1], to: p[2], test: !!p[3] })) };
  }

  // Groups crate links on the side that repeats more, so a long chain reads as a few lines.
  function crateLines(pairs) {
    const by = (key) => pairs.reduce((g, p) => g.set(p[key], (g.get(p[key]) || []).concat(p)), new Map());
    const crate = (n, test) => `<code>${esc(n)}</code>${test ? ' <span class="t">test</span>' : ""}`;
    const src = by("from"), dst = by("to");
    const lines = dst.size < src.size
      ? [...dst].map(([to, ps]) => `${ps.map((p) => crate(p.from, p.test)).join(", ")} → ${crate(to)}`)
      : [...src].map(([from, ps]) => `${crate(from)} → ${ps.map((p) => crate(p.to, p.test)).join(", ")}`);
    return `<ul class="crates">${lines.map((l) => `<li>${l}</li>`).join("")}</ul>`;
  }

  function cycleRow(p) {
    const c = parseCycle(p.message);
    if (!c) return `<li>${esc(p.message)}</li>`;
    const n = c.pairs.length;
    const row = (tag) => `<${tag} class="prow"><span class="pair"><span class="nm">${esc(c.from)}</span> → <span class="nm">${esc(c.to)}</span></span>` +
      `<span class="cnt">${n} crate link${n === 1 ? "" : "s"}</span></${tag}>`;
    return n <= 3 ? `<li>${row("div")}${crateLines(c.pairs)}</li>`
      : `<li><details>${row("summary")}${crateLines(c.pairs)}</details></li>`;
  }

  function problemList(sel) {
    const list = sel.problems || [];
    const peers = cycleOf.has(sel.name) ? MAP.cycles[cycleOf.get(sel.name)].filter((n) => n !== sel.name) : [];
    if (!list.length && !peers.length) return "";
    const groups = new Map(Object.keys(RULE_LABELS).map((r) => [r, []]));
    list.forEach((p) => groups.set(p.rule, (groups.get(p.rule) || []).concat(p)));
    let h = "";
    for (const [rule, items] of groups) {
      if (rule === "cycle" ? !items.length && !peers.length : !items.length) continue;
      let body;
      if (rule === "cycle") {
        const with_ = peers.length
          ? `<p class="with">With ${peers.map((n) => `<button data-name="${esc(n)}">${esc(n)}</button>`).join(", ")}</p>` : "";
        body = with_ + `<ul>${items.map(cycleRow).join("")}</ul>`;
      } else {
        body = `<ul>${items.map((p) => `<li>${esc(p.message)}</li>`).join("")}</ul>`;
      }
      const n = items.length ? `<span class="n">${items.length}</span>` : "";
      h += `<div class="pgroup ${rule === "cycle" ? "cycle" : ""}"><h4>${esc(RULE_LABELS[rule] || rule)}${n}</h4>${body}</div>`;
    }
    return `<section class="problems" aria-label="Problems"><h3>Problems <span class="n">${list.length}</span></h3>${h}</section>`;
  }

  function renderFocus() {
    const sel = byName.get(state.selected);
    const live = liveEdges();
    const pick = (list, key) => {
      const all = list.map((e) => ({ s: byName.get(e[key]), e }));
      const shown = all.filter((x) => visible(x.s))
        .sort((a, b) => areaIdx.get(a.s.area) - areaIdx.get(b.s.area) || a.s.name.localeCompare(b.s.name));
      return { shown, hidden: all.length - shown.length };
    };
    const uses = pick(live.filter((e) => e.from === sel.name), "to");
    const usedBy = pick(live.filter((e) => e.to === sel.name), "from");
    focusModel = { uses: uses.shown, usedby: usedBy.shown };

    const center = `<article class="center-card">
      <div class="top"><span class="badge ${sel.status === "approved" ? "approved" : "draft"}">${esc(sel.status)}</span>
        <span class="badge kind">${esc(sel.kind)}</span><span class="area">${esc(sel.area)}</span></div>
      <h2>${esc(sel.name)}</h2>
      <p>${esc(sel.purpose)}</p>
      <a class="doc-link" href="${esc(docUrl(sel.name))}" target="_blank" rel="noopener">Design doc</a>
      <dl class="facts"><div><dt>Lines</dt><dd>${fmt(sel.lines)}</dd></div>
        <div><dt>Violations</dt><dd class="${sel.violations ? "bad" : ""}">${fmt(sel.violations)}</dd></div></dl>
      </article>${problemList(sel)}`;

    $("#view-focus").innerHTML = `<div class="focus" id="focus"><svg class="wires" aria-hidden="true"></svg>` +
      column("uses", "Uses", uses.shown, uses.hidden, "Uses no other system.") + center +
      column("usedby", "Used by", usedBy.shown, usedBy.hidden, "No system uses it.") + "</div>";

    const root = $("#focus");
    root.addEventListener("mouseover", (ev) => {
      const card = ev.target.closest(".nb");
      root.classList.toggle("hovering", !!card);
      root.querySelectorAll(".wire.hot").forEach((w) => w.classList.remove("hot"));
      if (card) root.querySelectorAll(`svg.wires .wire[data-key="${card.closest(".col").classList[1]}:${card.dataset.name}"]`)
        .forEach((w) => w.classList.add("hot"));
    });
    root.addEventListener("mouseleave", () => root.classList.remove("hovering"));
  }

  function drawFocusWires() {
    const root = $("#focus");
    if (!root || !focusModel) return;
    const svg = $("svg.wires", root);
    const problems = $(".problems", root);
    if (getComputedStyle(svg).display === "none") {
      if (problems) problems.style.marginTop = "";
      svg.innerHTML = "";
      return;
    }
    // The panel sits in the row below the neighbours; pull it up under the card so it never moves the card.
    // Set it before sizing the SVG: it changes the height of the focus grid.
    if (problems) {
      const gap = $(".center-card", root).getBoundingClientRect().bottom - $(".col.uses", root).getBoundingClientRect().bottom;
      problems.style.marginTop = `calc(${gap}px + var(--panel-gap))`;
    }
    const base = sizeSvg(svg, root);
    const c = $(".center-card", root).getBoundingClientRect();
    const cTop = c.top - base.top;
    let out = "";
    for (const side of ["uses", "usedby"]) {
      const cards = root.querySelectorAll(`.col.${side} .nb`);
      cards.forEach((el, i) => {
        const r = el.getBoundingClientRect();
        const e = focusModel[side][i].e;
        const anchor = cTop + c.height * (0.2 + 0.6 * (i + 0.5) / cards.length);
        const mid = r.top - base.top + r.height / 2;
        const d = side === "uses"
          ? curve(c.left - base.left, anchor, r.right - base.left + 1, mid)
          : curve(r.left - base.left, mid, c.right - base.left + 1, anchor);
        out += `<path class="wire ${kinds(e)}" data-key="${side}:${esc(el.dataset.name)}" d="${d}" marker-end="url(#${marker(e)})"/>`;
      });
    }
    svg.innerHTML = out;
  }

  // ---- Matrix view -------------------------------------------------------
  function groupByArea(list) {
    return areas.map((a) => ({ area: a, items: list.filter((s) => s.area === a) })).filter((g) => g.items.length);
  }

  function renderMatrix() {
    const vis = visibleSystems();
    const host = $("#view-matrix");
    if (!vis.length) { host.innerHTML = '<p class="empty-view">No system matches the filters.</p>'; return; }
    const em = new Map(liveEdges().map((e) => [e.from + "\u0000" + e.to, e]));
    const groups = groupByArea(vis);
    const starts = new Set();
    let k = 0;
    groups.forEach((g) => { starts.add(k); k += g.items.length; });
    const sel = state.selected;

    let h = '<table class="mx"><thead><tr><th class="corner" rowspan="2">row uses column</th>';
    h += groups.map((g) => `<th class="ag" colspan="${g.items.length}" title="${esc(g.area)}">${esc(g.area)}</th>`).join("");
    h += "</tr><tr>";
    h += vis.map((s, j) => `<th class="cl${starts.has(j) ? " gs" : ""}${s.name === sel ? " selc" : ""}" data-c="${j}" data-name="${esc(s.name)}" title="${esc(s.name)}"><span>${esc(s.name)}</span></th>`).join("");
    h += "</tr></thead><tbody>";
    let i = 0;
    for (const g of groups) {
      h += `<tr class="grp"><th>${esc(g.area)}</th><td colspan="${vis.length}"></td></tr>`;
      for (const r of g.items) {
        const rs = r.name === sel ? " selr" : "";
        h += `<tr data-r="${i}"><th class="rl${rs}" data-name="${esc(r.name)}">${esc(r.name)}${isProblem(r) ? '<i class="pdot" title="Has problems"></i>' : ""}</th>`;
        vis.forEach((c, j) => {
          const e = em.get(r.name + "\u0000" + c.name);
          const cls = [starts.has(j) && "gs", i === j && "diag", rs && "selr", c.name === sel && "selc"].filter(Boolean).join(" ");
          h += `<td data-c="${j}"${cls ? ` class="${cls}"` : ""}>${e ? `<i class="m ${kinds(e)}"></i>` : ""}</td>`;
        });
        h += "</tr>";
        i++;
      }
    }
    h += "</tbody></table>";
    host.innerHTML = `<div class="mx-wrap">${h}</div>`;

    const table = $("table.mx", host);
    const rowsEls = [...table.querySelectorAll("tbody tr[data-r]")];
    const cols = vis.map(() => []);
    table.querySelectorAll("[data-c]").forEach((el) => cols[+el.dataset.c].push(el));
    const tip = $("#tip");
    let cur = null;
    const clear = () => {
      if (!cur) return;
      cur.row && cur.row.querySelectorAll("th, td").forEach((x) => x.classList.remove("hr"));
      cur.col && cur.col.forEach((x) => x.classList.remove("hc"));
      cur = null;
      tip.hidden = true;
    };
    table.addEventListener("mousemove", (ev) => {
      const cell = ev.target.closest("td[data-c], th.rl, th.cl");
      const tr = cell && cell.closest("tr[data-r]");
      const r = tr ? +tr.dataset.r : null;
      const c = cell && cell.dataset.c !== undefined ? +cell.dataset.c : null;
      if (!cur || cur.r !== r || cur.c !== c) {
        clear();
        cur = { r, c, row: r !== null ? rowsEls[r] : null, col: c !== null ? cols[c] : null };
        cur.row && cur.row.querySelectorAll("th, td").forEach((x) => x.classList.add("hr"));
        cur.col && cur.col.forEach((x) => x.classList.add("hc"));
      }
      const e = r !== null && c !== null && em.get(vis[r].name + "\u0000" + vis[c].name);
      if (!e || cell.tagName !== "TD") { tip.hidden = true; return; }
      tip.innerHTML = describe(e);
      tip.hidden = false;
      const tw = tip.offsetWidth, th = tip.offsetHeight;
      tip.style.left = Math.min(ev.clientX + 14, innerWidth - tw - 8) + "px";
      tip.style.top = (ev.clientY + 16 + th > innerHeight ? ev.clientY - th - 10 : ev.clientY + 16) + "px";
    });
    table.addEventListener("mouseleave", clear);
  }

  // ---- Overview ----------------------------------------------------------
  function renderOverview() {
    const vis = visibleSystems();
    const host = $("#view-overview");
    if (!vis.length) { host.innerHTML = '<p class="empty-view">No system matches the filters.</p>'; return; }
    const cols = groupByArea(vis).map((g) => `<div class="ov-col"><h3>${esc(g.area)} <span class="n">${g.items.length}</span></h3><div class="chips">` +
      g.items.map((s) => `<button class="chip" data-name="${esc(s.name)}" title="${esc(s.name)}: ${esc(s.purpose)}">` +
        `<span class="cname">${esc(s.name)}</span>${cycleOf.has(s.name) ? '<i class="ring" title="in a cycle"></i>' : ""}` +
        `${s.status === "approved" ? '<i class="ok" title="approved"></i>' : ""}` +
        `${s.violations ? `<i class="vio" title="${s.violations} violations">${s.violations}</i>` : ""}</button>`).join("") +
      "</div></div>").join("");
    host.innerHTML = '<p class="ov-hint">Hover a system to see its edges. Click to select it, double-click to open it.</p>' +
      `<div class="ov" id="ov"><svg class="wires" aria-hidden="true"></svg><div class="ov-grid">${cols}</div></div>`;
    const root = $("#ov");
    root.addEventListener("mouseover", (ev) => {
      const chip = ev.target.closest(".chip");
      const n = chip ? chip.dataset.name : null;
      if (n !== state.hover) { state.hover = n; drawOverviewWires(); }
    });
    root.addEventListener("mouseleave", () => { state.hover = null; drawOverviewWires(); });
  }

  function drawOverviewWires() {
    const root = $("#ov");
    if (!root) return;
    const svg = $("svg.wires", root);
    const active = state.hover || state.selected;
    const chips = new Map([...root.querySelectorAll(".chip")].map((el) => [el.dataset.name, el]));
    chips.forEach((el) => el.classList.remove("on", "uses", "usedby"));
    const base = sizeSvg(svg, root);
    const on = chips.get(active);
    root.classList.toggle("active", !!on);
    if (!on) { svg.innerHTML = ""; return; }
    on.classList.add("on");
    const box = (el) => {
      const r = el.getBoundingClientRect();
      return { l: r.left - base.left, r: r.right - base.left, y: r.top - base.top + r.height / 2 };
    };
    let out = "";
    for (const e of liveEdges()) {
      const dir = e.from === active ? "uses" : e.to === active ? "usedby" : null;
      const other = dir === "uses" ? e.to : e.from;
      if (!dir || !chips.has(other) || other === active) continue;
      chips.get(other).classList.add(dir);
      const a = box(chips.get(e.from)), b = box(chips.get(e.to));
      let d;
      if (Math.abs(a.l - b.l) < 4) {
        const x = a.r, bulge = 18 + Math.min(40, Math.abs(a.y - b.y) / 6);
        d = `M${x},${a.y} C${x + bulge},${a.y} ${x + bulge},${b.y} ${x + 1},${b.y}`;
      } else if (b.l > a.l) {
        d = curve(a.r, a.y, b.l - 1, b.y);
      } else {
        d = curve(a.l, a.y, b.r + 1, b.y);
      }
      out += `<path class="wire ${dir}${e.test ? " test" : ""}" d="${d}" marker-end="url(#ah-${dir})"/>`;
    }
    svg.innerHTML = out;
  }

  // ---- Render ------------------------------------------------------------
  function drawWires() {
    if (state.view === "focus") drawFocusWires();
    else if (state.view === "overview") drawOverviewWires();
  }

  function render() {
    document.querySelectorAll(".tabs button").forEach((b) => b.setAttribute("aria-selected", String(b.dataset.view === state.view)));
    for (const v of ["focus", "matrix", "overview"]) $("#view-" + v).hidden = v !== state.view;
    $("#tip").hidden = true;
    state.hover = null;
    renderTrail();
    renderLegend();
    if (state.view === "focus") renderFocus();
    else if (state.view === "matrix") renderMatrix();
    else renderOverview();
    drawWires();
  }

  initControls();
  const fromHash = decodeURIComponent(location.hash.slice(1));
  select(byName.has(fromHash) ? fromHash : defaultName);
})();
