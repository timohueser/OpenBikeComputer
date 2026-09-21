# Documentation authoring

`docs/content/` is the source of the guide at openbikecomputer.com. `build_docs.py` renders it
with the Python standard library; `content/nav.json` is the page order and title authority. Blog
posts are described in [BLOG.md](BLOG.md).

- A page explains how a part of the product works and why. Byte and wire contracts stay in
  `specs/`; build and run instructions stay in the README next to the code.
- Link to code with `[text](src:path)`, never a line number.
- Check every technical claim against the current source.

## Diagrams

Diagrams are inline SVG in the Markdown; screen captures and hardware renders are image assets.
Follow the existing diagrams in [formats](content/software/formats.md) and
[ui](content/software/ui.md): a 720-unit viewBox, the `d-*` classes, forest for structure,
coral for the hot path, amber for rider or route emphasis, colour never the only distinction.
Every SVG has an `aria-label`, a caption outside the drawing, and a `diagram-scroll` wrapper with
`--diagram-width` set to its viewBox width. After a change, check the rendered page at desktop and
phone width.

## Copy ownership

Every page declares `copy: ai`, `mixed` or `human` in its front matter. On a `mixed` page, human
prose sits between `<!-- human-copy:start -->` and `<!-- human-copy:end -->`. Do not rewrite
human-owned prose. When it is stale, add a non-rendered note beside it with the current facts and
their source, and report it in the pull request:

```md
<!-- copy-review:
The device now reads terrain from the combined OBCM file. See firmware/obc-reader/src/...
-->
```

`obc docs` lists the pages by ownership and every pending note; `obc docs check` validates the
front matter and markers. `obc docs review --base origin/develop` lists the pages that link to
files a change touched.

## Check and preview

```sh
python3 docs/build_docs.py --check-links
python3 docs/serve.py "$PWD/docs" 8090   # then open http://127.0.0.1:8090/docs/
```

The preview does not build the WebAssembly landing-page demo.
