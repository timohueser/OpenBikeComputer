---
system: website
kind: shell
purpose: "Publishes the project site, the guide and the blog."
owns:
  - docs/**
depends_on: []
test_depends_on: []
status: approved
---

# Website

## What it does

The website is the project site at openbikecomputer.com: the landing page, the guide, the blog, the system docs and the hardware parts list. The script [`build_docs.py`](../../docs/build_docs.py) renders the Markdown pages of `docs/content/` and the blog posts to HTML. [`nav.json`](../../docs/content/nav.json) gives the order and the titles of the guide pages. The script also makes the data-sources tables from the data registry, a page for each system doc, and, with the `--hardware` flag, the parts list of the hardware.

Trunk ([`Trunk.toml`](../../docs/Trunk.toml)) builds the landing page with the web demo of the simulators, and runs the script first. The site deploy ([`deploy-site.yml`](../../.github/workflows/deploy-site.yml)) adds the map builder at `/builder/` and the route planner at `/plan/`. [`serve.py`](../../docs/serve.py) shows the site on this machine. `docs/README.md` gives the authoring rules.

## Rules that must hold

| Rule | Check |
| --- | --- |
| Every link between pages goes to a page and a heading that exist. | `--check-links` of [`build_docs.py`](../../docs/build_docs.py), through `check-docs` in [`justfile`](../../tools/justfile) |
| The footers of the hand-written pages show the credits of the data registry. | `test_the_static_page_footers_show_the_registry_credits` in [`test_data_registry.py`](../../tools/tests/test_data_registry.py) |
| Each download of the hardware page exists. | `test_downloads_must_exist_and_page_fragments_still_need_headings` in [`test_hardware_docs.py`](../../tools/tests/test_hardware_docs.py) |
| The site works under any path of a server. | The step `Mountable at any prefix` in [`deploy-site.yml`](../../.github/workflows/deploy-site.yml) |

## Decisions

- **The renderer uses only the Python standard library.** So the site build cannot fail on a missing package, and the Trunk hook can run it on any machine.
- **The data-sources tables come from the registry.** A `sources` block in a page renders a table of `data/sources.toml`, so the page cannot differ from the registry.
