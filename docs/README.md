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

The rendering-pipeline page is the reference for depth and visual style. Run the link checker
before publishing:

```sh
python3 docs/build_docs.py --check-links
```

Blog folders, front matter, comparison images and 3D models are documented in
[`BLOG.md`](BLOG.md).
