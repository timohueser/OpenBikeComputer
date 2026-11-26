# Docs

The public site, openbikecomputer.com: the landing page with the live device demo
(`index.html`), the guide (`content/`, published at `/docs/`), the blog (`content/blog/`,
published at `/blog/`) and the legal pages. `testing.md` is not a site page: it explains the
test plan of the repository.

Not here: the web planner at `/plan/` (`builder/web`; the deploy copies it in), the byte
contracts (`specs/`), and the code of the landing-page demo (`sim/web-demo`).

## Where to start

| Path | What |
| --- | --- |
| `build_docs.py` | Renders the guide and the blog, and checks the links. Python standard library only. Its docstring lists the supported Markdown. |
| `content/nav.json` | The sections, order and titles of the guide. A page is published only if this file lists it. |
| `content/` | The guide pages and, under `blog/<slug>/`, the posts. |
| `templates/`, `assets/` | The page shells, the shared header `_sitehead.html`, the CSS, the fonts, and the diagrams in `assets/diagrams/`. |
| `index.html`, `Trunk.toml` | The landing page and the site build. Trunk runs `build_docs.py` first, then builds `sim/web-demo` to WebAssembly. |
| [BLOG.md](BLOG.md) | How to write a post: scaffold, images, the `compare` and `model` fences. |
| [testing.md](testing.md) | Test routes, `suites.toml` and CI artifacts. |

## Run and test

```sh
obc check-docs                               # python3 docs/build_docs.py --check-links
python3 docs/serve.py "$PWD/docs" 8090       # then open http://127.0.0.1:8090/docs/
trunk serve --config docs/Trunk.toml         # the whole site with the live demo, on :8080
python3 docs/build_docs.py --hardware --check-links   # with the parts list; needs KiCad 10
```

`serve.py` does not build the landing-page demo. `obc site` builds the web planner, not this site.

`--check-links` checks the links between rendered pages, including each `#fragment`. It does not
see `src:` links or pages that `nav.json` leaves out; `tools/check_paths.py` checks those, and
the relative links of every other Markdown file.

`.github/workflows/deploy-site.yml` publishes one GitHub Pages artifact on a push to `develop`:
this site, the hardware exports and the web planner under `/plan/`. The suite `ci.docs` runs
`obc check-docs`; `web.demo-browser` tests the landing page.

## Writing a guide page

- A page explains how a part of the product works and why. Byte and wire contracts stay in
  `specs/`: link the spec, do not restate its tables. Build and run instructions stay in the
  README next to the code.
- Link to code with `[text](src:path)`, never a line number. The link goes to the file on
  `develop`.
- Check every technical claim against the current source.
- The tables of `content/software/data-sources.md` come from `data/sources.toml` through
  ```` ```sources ```` fences. Change the registry, not the page.
- A change to the public docs is its own `docs:` commit.

## Diagrams

Diagrams are SVG assets in `assets/diagrams/`. Use a relative
`<img src="..." alt="..." data-inline-svg>` tag. The renderer embeds the asset in the page.
Screen captures and hardware renders use normal image tags.
Follow the existing diagrams in [formats](content/software/formats.md) and
[ui](content/software/ui.md): a 720-unit viewBox, the `d-*` classes, forest for structure,
coral for the hot path, amber for rider or route emphasis, colour never the only distinction.
Every SVG has an `aria-label`, a caption outside the drawing, and a `diagram-scroll` wrapper with
`--diagram-width` set to its viewBox width. After a change, check the rendered page at desktop and
phone width.

## Gotchas

- `docs/docs/` and `docs/blog/` are build output. Each build deletes them. Their `.gitkeep`
  files must stay, because Trunk resolves its watch-ignore paths before the build hook runs.
- A `nav.json` entry with no file gives a warning, not a failure.
- Raw HTML passes through only when the tag starts at column 0.
- Every published URL is relative. The deploy fails on a baked-in site path and serves the
  artifact under a deep prefix to prove it. Only the `og:image` and `twitter:image` tags of the
  landing page hold an absolute URL.
- GitHub Pages sends no COOP or COEP headers and caches for ten minutes. The site has no
  `SharedArrayBuffer`, and nothing that changes often can live in the artifact.
- The hardware export runs only with `--hardware` or `OBC_BUILD_HARDWARE=1`. The deploy sets it.
- A blog post needs `title:` and an ISO `date:` in its front matter, or the build stops.

## Decisions

- The renderer uses only the standard library, so the site build cannot fail on a missing
  package and the Trunk hook runs anywhere.
- The landing-page demo is the firmware render path compiled to WebAssembly. Each push rebuilds
  it, so the site never shows a stale screenshot.
- One Pages artifact, deployed from `develop` only. Two deploying branches would fight over one
  Pages environment.
- Relative URLs only, so a move to another domain or path costs a DNS record.
- The header is one shared partial, so the guide and the blog cannot differ.
- The `compare` and `model` blocks work without JavaScript; the script only improves them.
