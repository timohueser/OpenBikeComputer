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
