# Writing a log entry

The blog ("expedition log") is markdown in `docs/content/blog/<slug>/index.md`. The docs build
(`build_docs.py`, standard library only) renders it, and the same Trunk build publishes it at
`/blog/`. One folder per post; images and models sit next to the markdown and are copied verbatim
next to the rendered page.

## The loop

```sh
python3 docs/tools/blog_new.py "My post title"     # scaffold the folder + front matter
trunk serve --config docs/Trunk.toml               # live preview at 127.0.0.1:8080/blog/
python3 docs/tools/paste_images.py my-post-title   # paste/drop images -> post folder
```

`trunk serve` re-renders on every save (the `pre_build` hook) and reloads the browser. The paste
tool (port 8091) saves clipboard images or dropped files into the post folder, downscaled to
1600 px if Pillow is installed, and puts the matching `![](file.png)` line on your clipboard.

## Front matter

```yaml
---
title: The post title            # required
date: 2026-07-21                 # required, YYYY-MM-DD — orders the timeline
description: One-line teaser.    # shown on the timeline + in the Atom feed
---
```

The h1 comes from `title:`: start the body with a paragraph, not a heading. The first paragraph
renders as the lede. `/blog/` sorts posts newest first and groups them by month; the Atom feed is
`/blog/feed.xml`.

The renderer strips HTML comments (`<!-- … -->` at the start of a line) from the page. Use them
for drafts and notes, or to park an example without publishing it, as the scaffold does.

## Images

A paragraph with only one image becomes a framed, click-to-zoom exhibit. The alt text is the
lightbox caption:

```markdown
![The first assembled unit](assembly.png)
```

## Before/after slider

````markdown
```compare
before: rev-a.jpg
after: rev-b.jpg
label-before: Rev A        # optional, defaults Before/After
label-after: Rev B
caption: What changed.     # optional
```
````

## Interactive 3D models

Convert a STEP export once. Commit the `.glb` next to `index.md`, and the `.step` too if you want
the download button:

```sh
pip install cascadio    # one-time; prebuilt OpenCascade wheels
python3 docs/tools/step2glb.py case.step docs/content/blog/<slug>/case.glb
```

````markdown
```model
glb: case.glb
step: case.step            # optional -> "Download STEP" button
caption: Drag to orbit.    # optional
```
````

The viewer (`assets/glb-viewer.js`) reads only the subset that the converter writes. Keep models
under about 4 MB; coarsen `--linear` if necessary.

## Publishing

Commit the post folder and push. CI renders and link-checks the whole site;
`python3 docs/build_docs.py --check-links` runs the same check locally.
