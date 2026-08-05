# Why the docs are shaped this way

The record of the decisions behind `/docs/` — kept because they still bind every page
written into it. Not published; the site itself is the output.

The docs are a *conceptual* companion to the code, **not** an API reference. The code is
already meticulously documented in-source; the docs exist to explain the **core concepts,
the philosophy, and the load-bearing boundaries** — the things a newcomer can't easily
reconstruct by reading files one at a time.

- **Authoring:** human-editable **Markdown** sources rendered to themed HTML by a small
  **stdlib-only Python** script (`build_docs.py`), so the build never breaks on a missing
  package. `content/nav.json` is the nav manifest (sections, page order, titles).
- **Code references:** concept + philosophy + diagrams, with lightweight "where to look"
  **file links** — the `[src:path]` shorthand, **no line numbers**, they rot — and the
  **occasional inline snippet** where it genuinely clarifies. Verify every claim against
  source before writing it down; the design notes under `firmware/docs/` are point-in-time
  drafts and have drifted.
- **Diagrams:** "**field-guide blend**" — technically precise (data layouts, pipeline
  flows, state machines) but wearing the site's skin. Bespoke inline SVG embedded straight
  in the markdown, no diagram-as-code.
- **Visual system:** the landing page's own tokens, so the docs feel like the same world —
  parchment `#ece8cf` / `#e4dec0` / `#d6cda8`, ink `#24331c`, forest `#3c6b39`, wood
  `#5f7d3d`, amber `#e3ad33`, coral `#cf6a2a`; serif (Iowan) headings, sans body, mono for
  code and labels; the drifting topo contours as a faint backdrop.
- **Diagram conventions**, so every figure reads as one hand:
  - parchment panels with thin forest/ink strokes; coral = "the hot path / the thing to
    notice"; amber = the user/route accent; muted greens/blues for map features.
  - mono labels, small-caps section tags, hand-numbered call-outs ①②③ like a plate.
  - flows read left→right or top→down with clear arrowheads; data layouts drawn as
    labeled byte cells; state machines as rounded nodes + labeled edges.
  - every figure carries a one-line italic caption and a real `aria-label`.

The **rendering pipeline** page is the bar every other page is written up to: concepts,
figures, and a few well-chosen snippets. `--check-links` audits every cross-page anchor
against the real heading ids and exits non-zero on a break; CI runs it.

Blog authoring — the per-post folders, the ```compare / ```model directives — is its own
guide: [BLOG.md](BLOG.md).
