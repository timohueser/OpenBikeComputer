# Documentation authoring

`docs/content/` is the source for the conceptual guide at openbikecomputer.com. It explains
system boundaries and design choices; API and implementation detail stay in source comments and
the normative contracts under `specs/`.

- `build_docs.py` renders the Markdown with only the Python standard library.
- `content/nav.json` is the page order and title authority.
- Link to code with the `[src:path]` shorthand. Avoid line numbers, which rot quickly.
- Verify technical claims against current source. Point-in-time notes under `firmware/docs/` are
  supporting references, not a second implementation authority.
- Embed accessible SVG diagrams directly in Markdown. Flows read left-to-right or top-to-bottom,
  every figure has a caption, and every SVG has a useful `aria-label`.
- Reuse the landing-page visual tokens: parchment surfaces, ink/forest structure, coral for the
  hot path, and amber for rider or route emphasis.

Run the link checker before publishing:

```sh
python3 docs/build_docs.py --check-links
```

Blog folders, front matter, comparison images and 3D models are documented in
[`BLOG.md`](BLOG.md).

## Current guidance and implementation history

[plans.md](plans.md) indexes active implementation plans and remaining acceptance work. Add complex
work there when it starts. When it ends, remove its active row and mark its handoff **Historical**
with a link to current guidance. Keep past decisions and measurements in the historical record.
A partly complete plan must state the work that remains; do not infer physical acceptance from CI.

Before a PR, run `obc docs review --base origin/develop`. For a weekly maintenance pass, run
`obc docs review --since 1.week`. These commands list the nearest README and Markdown pages that
link to changed files or directories. Review those candidates against source, check the active
plan index, and update stale guidance. Use the copy-review process below for protected prose.
The queue is a local read-only aid, not a scheduled agent or proof that the prose is correct.
It cannot find claims that have no source link. Check those during review and add useful links.

## Diagram style

Use inline SVG for diagrams. Keep screen captures and hardware concepts as separate image assets.
Do not redraw a screen capture to make it match the documentation palette.

- Preserve the visual explanation: byte rulers show field widths and offsets, spatial diagrams
  show geometry, and process diagrams show the relevant steps. Do not replace these with
  summary boxes. Remove clutter without removing the information needed to reason about the system.
- Use a 720-unit viewBox where possible. Flow left to right or top to bottom.
- Use `d-title` for box headings, `d-label` for labels, `d-sub` for short notes, and `d-tag` for
  the figure title. Use the shared CSS font sizes. Do not reduce labels to make them fit.
- Use `d-panel` for nodes and `d-flow` for arrows. Use `d-focus` on a panel to emphasize a key
  step. Put arrowheads only at a connection's destination, not at bends or branch joins.
  Keep arrowheads outside boxes and route connectors clear of text.
- Use forest for structure, coral for the selected path, and amber for rider or route emphasis.
  Label states and paths so that color is never the only distinction.
- Label examples and schematics. State when a byte or memory layout is not to scale.
  Byte ranges, visible labels, captions, and accessible descriptions must agree.
- Give every SVG a useful `aria-label`, unique marker IDs within the page, and a caption that
  explains the result. Keep long explanations out of the drawing.
- Wrap each SVG in a focusable `diagram-scroll` region. Set `--diagram-width` to its viewBox
  width in pixels. This preserves label size on small screens. Keep the caption outside the
  scrolling region. Use the existing `diagram-hint` below it.

The [map header](content/software/formats.md) and [rider paths](content/software/ui.md) show
these conventions. Diagrams stay in Markdown; no diagram generator or image service is required.

After a diagram change, inspect its rendered page at desktop and phone widths. Check labels,
box boundaries, arrow directions, captions, keyboard scrolling, and page overflow. A link check
alone does not check the image layout.

For a local preview, build the pages, then run this from the repository root:

```sh
python3 docs/serve.py "$PWD/docs" 8090
```

Open `http://127.0.0.1:8090/docs/`. This preview does not build the WebAssembly landing-page demo.

## Copy ownership

Every Markdown page under `docs/content/` declares the state of its user-facing prose in front
matter:

```yaml
copy: ai
```

- `ai` means an agent can rewrite the prose.
- `mixed` means only marked passages are human-owned.
- `human` means all prose on the page is human-owned.

On a mixed page, wrap each human-owned passage in non-rendered authoring comments:

```md
<!-- human-copy:start -->
Human-written text.
<!-- human-copy:end -->
```

Do not rewrite human-owned prose when it becomes stale. Add a non-rendered note beside it with the
current facts and their source, then report the note in the pull request:

```md
<!-- copy-review:
The device now reads terrain from the combined OBCM file.
See firmware/obc-reader/src/...
-->
```

Run `obc docs` to list AI drafts, mixed and human pages, and pending review notes. Run
`obc docs check` to validate the front matter and markers. The normal documentation gate also runs
this validation.

`obc docs check` rejects an empty note, but it cannot tell whether a note gives its source, because
the note is free prose. The facts and the source stay the author's responsibility. `obc docs` prints
each note in full, so a reviewer can see when one has no source.
