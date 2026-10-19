# Europe tune

The tune gives each baseline ecoregion of Europe its rules and assets, so that Trail View looks
right everywhere. The baseline is the 45 regions in `tune/ecoregions.json` (RESOLVE Ecoregions
2017, Europe to 60° E, without the regions only in Russia). Each region has a short name, such as
`alps` or `celtic`.

One tracking issue on GitHub holds the batch plan and the asset census, and the demo shows the
status of each region. This file holds only the instructions.

## Linux setup

One Linux machine bakes and runs the agents of the Europe tune. Plan at least 32 GB RAM and
300 GB disk. Run the agents as a normal user, not root.

1. Packages. `python3-gdal` brings `gdal_proximity.py`. The bakes so far ran on GDAL 3.13 on the
   Mac; Ubuntu 24.04 brings 3.8, and the smoke test in step 9 shows whether it works.

   ```sh
   sudo apt install -y git curl ca-certificates gdal-bin python3-gdal build-essential gh
   ```

2. uv:

   ```sh
   curl -LsSf https://astral.sh/uv/install.sh | sh
   source ~/.local/bin/env          # or open a new shell
   ```

3. Node 24 LTS, Claude Code, GitHub and a git identity:

   ```sh
   curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash
   source ~/.nvm/nvm.sh && nvm install 24
   npm install -g @anthropic-ai/claude-code
   claude                      # log in, then quit
   gh auth login
   git config --global user.name "<name>"
   git config --global user.email "<email>"
   ```

4. The whole repository: the page imports `builder/web`, and bake.py imports `host/obc-dem`.

   ```sh
   git clone https://github.com/timohueser/OpenBikeComputer.git
   cd OpenBikeComputer && git checkout archive/trail-view-prototype
   python3 tools/fixtures.py sync peak-view-photos   # the photographs of the Alps spots
   ```

5. The CDSE keys, from the Mac. `box` is the Linux machine's SSH alias (`Host box` in
   `~/.ssh/config`). Never print the file.

   ```sh
   ssh box mkdir -p .config/openbikecomputer
   scp ~/.config/openbikecomputer/cdse-s3.env box:.config/openbikecomputer/
   ssh box chmod 600 .config/openbikecomputer/cdse-s3.env
   ```

6. Optional, from the Mac: the bakes and source caches, about 8 GB of downloads less. The Mac's
   bakes are current for this branch. Without them, step 10 bakes every spot once.

   ```sh
   ssh box mkdir -p .cache/obc/trail-view
   rsync -a --exclude shots --exclude review ~/.cache/obc/trail-view/ box:.cache/obc/trail-view/
   ```

7. The page and headless Chromium, in `prototype/trail-view`:

   ```sh
   npm ci && npx playwright install --with-deps chromium
   ```

8. A GPU only if the machine has one: its vendor driver, then `sudo apt install -y libvulkan1
   vulkan-tools` and `vulkaninfo --summary`. Without a GPU, add `--swiftshader` to every
   `npm run shoot`. It renders the same picture on the CPU at about 2 s a frame.

9. Smoke test, from the repository root. The JSON line names the WebGL `renderer`. If it names
   SwiftShader on a machine with a GPU, the Vulkan path does not work: use `--swiftshader` and
   tell the owner.

   ```sh
   uv run prototype/trail-view/bake.py healy-pass
   cd prototype/trail-view && npm run shoot -- healy-pass --time
   ```

10. Only when the caches did not come from the Mac, or a bake's `meta.json` has no `region`: bake
    every spot once, from the repository root. A rebake keeps the colour and the OSM answers, so
    it takes about a minute a spot.

    ```sh
    for id in $(python3 -c "import json, glob; print(' '.join(s['id'] for f in sorted(glob.glob('prototype/trail-view/spots/*.json')) for s in json.load(open(f))))"); do uv run prototype/trail-view/bake.py "$id"; done
    ```

## Start

On the Linux box, finish the setup above. Then start one Claude Code session in the clone from
step 4, which is the coordinator's checkout, and give it this message:

> Read prototype/trail-view/tune/README.md and act as the coordinator of the Europe tune.

Run bake.py from the repository root. Run every other command in `prototype/trail-view`.

The coordinator works in this order and reports to the owner in its chat after each step:

1. **Demo.** Serve the demo (see "Demo"). Tell the owner how to open it, and wait for the
   owner's OK.
2. **Tracking issue.** Ask the owner, then open one GitHub issue, "Trail View: Europe tune", for
   the batch plan and the census. Fix the regression set: every spot in `spots/` at the start of
   the tune, and later every spot of an approved region.
3. **Phase 1** for every region, then the **census and batch 0**. Show the census and the batch
   plan to the owner before batch 0.
4. **Region batches**, at most four regions at a time, until all 45 are done.

## Roles

- **Owner.** Looks at the demo and gives feedback and approvals to the coordinator, in its chat.
  Registers API keys.
- **Coordinator.** The session in the clone, on `archive/trail-view-prototype`. It serves the
  demo, runs phase 1 and the census, plans the batches, starts the region agents, passes the
  owner's feedback to them, keeps the region status, lands their branches, owns the frozen files
  and renders the regression set after each shared change. It asks the owner before every push.
- **Region agent.** A background agent of the coordinator, on Opus, one per region, in its own
  worktree on branch `tune/<short>`. It writes only `regions/<short>.json`,
  `regions/<short>.*.json`, `packs/<short>.json`, `packs/<short>.*.json`, `spots/<short>.json`
  and the asset files that the census gave it. It reports to the coordinator, never to the owner.
- **Asset builder.** A region agent or a batch-0 agent that holds the claim for an asset.

## Phase 1: research, for every region

Phase 1 needs no bakes. All regions can run it, four at a time.

1. **Reference photos.** Find about 5 geolocated photos on Wikimedia Commons, more for a large
   or varied region. Choose views a bikepacker sees: valleys, passes, coasts, villages, forest
   roads. Send one Commons API request at a time, with bake.py's User-Agent, and use 1920 px
   thumbnails. Record the camera position, the heading if known, the author and the licence.
   Each photo becomes a spot in `spots/<short>.json` with `lat`, `lon` and a `photo` block:
   `url`, `credit`, `heading_deg`, `pitch_deg`, `vfov_deg` and `roll_deg`. bake.py stops without
   `heading_deg` and `pitch_deg`, so estimate them; the camera is aligned in the page after the
   bake. Photos stay in the cache. Never commit them.
2. **Species and culture.** List the typical trees, shrubs, ground plants, grazing animals and
   house styles, with Latin names. `npm run check -- --assets` lists the models that exist. Map
   each species to an existing model, or add a stand-in alias to the pack:
   `"pedunculate-oak": {"model": "european-beech", "latin": "Quercus robur", "leaf": "broadleaved",
   "cover": "tree", "elevation": [0, 900], "weight": 2}`.
3. **Split decision.** Avoid splits. First use the rules that exist: Sentinel-2 colour, the NDVI
   ramps (one per season where needed), the barren rule, tree elevation bands, snow data,
   `windswept_m`, `turf_deg`, and culture by country or subdivision. Split only where a key that
   applies at the eye (haze, weather, grade) must change across stable geography, such as an
   island side, a massif or a coast strip. Never draw contour rings. Ask the coordinator for a height rule
   instead. Write the decision and the reason into the region's `about` and the tracking issue.

The result of phase 1 is a region file with `about` and `leaf`, a spots file with photo spots,
and a pack that uses stand-in aliases.

## Census and batch 0

The coordinator does this once, after phase 1.

1. `npm run check -- --assets` prints the census: each stand-in alias, grouped by Latin name,
   with the regions that use it.
2. Give each missing asset exactly one builder. Write the table into the tracking issue, and
   create the claims (see below).
3. Batch 0 builds the assets that many regions share, for example oaks, ash, hornbeam, lime,
   sweet chestnut, alder, willow, poplar, the pines, olive, cypress, juniper, cattle, horses,
   the main house styles and a church.
4. The owner approves each asset on the model sheet (`models.html`) and in one spot.

## Region batches

Four regions run at a time. Mix landscapes in a batch, for example one boreal, one Atlantic,
one alpine and one Mediterranean region, so that the four share few assets. For each region, the
coordinator makes a worktree and serves it for the owner (slot N of 1 to 4):

```sh
git worktree add ../tune-<short> -b tune/<short> archive/trail-view-prototype
cd ../tune-<short>/prototype/trail-view && npm ci && npx vite --port 518N
```

Each region agent repeats these steps:

1. **Bake.** `uv run prototype/trail-view/bake.py <spot>` for each spot. Give a spot the best
   data that needs no key with its `dtm`, `landcover` and `geology` keys. See "Data sources".
2. **Tune.** Change the region's look and rules and the pack. `npm run check` must pass.
3. **Check.** `npm run shoot -- <spot>,<spot>` (with `--swiftshader` on a machine without a GPU).
   Look at the PNGs with the Read tool, against the reference photos, and fix what is clearly
   wrong before the owner sees it.
4. **Report.** Tell the coordinator what changed and why. The coordinator sets the region to
   "ready for feedback" and tells the owner, with the link to slot N's demo.
5. **Feedback.** The owner answers the coordinator. The coordinator passes the feedback to the
   same agent and sets the region back to "in progress". Start again at step 1 or 2.
6. **Land.** When the owner approves: `npm run check`, then one commit on `tune/<short>`. The
   coordinator rebases it onto `archive/trail-view-prototype`, fast-forwards, sets the region to
   "done" and starts the next region in the slot.

## Claims: each asset is built once

An asset is a tree, plant or animal model (`src/models/trees/`, `plants/` or `animals/<id>.ts`), a
house style (`styles/<id>.json`) or a building kind (`src/models/buildings.ts`). Its id is the
English name as a slug, for example `pedunculate-oak`. Its file holds the Latin name. One Latin
name has one model; `npm run check` fails on two. A breed or look of one species is a model of its
own, with the breed after the species: `Bos taurus (Highland)`, `Bos taurus (Holstein-Friesian)`.

- Before you build an asset, claim it:
  `mkdir -p ~/.cache/obc/trail-view/_claims/<kind> && mkdir ~/.cache/obc/trail-view/_claims/<kind>/<id>`,
  where kind is `tree`, `plant`, `animal`, `style` or `building`. The second `mkdir` is atomic.
  If it succeeds, write a file `owner` in it with your region, worktree and the time. If it fails,
  read `owner`, use a stand-in alias and continue. Do not build the asset.
- An asset commit holds only the asset. It lands when the owner approves it on the model sheet
  and in one spot. Then write a file `done` with the commit in the claim.
- An asset is frozen once a region that uses it is approved. A variant is a new file that
  derives from it.

## Frozen files

During a batch, region agents do not edit `regions/world.json`, `packs/base.json`, `bake.py`,
`materials.json`, `src/shaders.ts`, `src/terrain.ts`, `src/rules.ts`, `src/nearfield.ts`,
`src/palette.ts`, `src/light.ts`, `src/weather.ts`, `src/regions.ts` or `src/models/shape.ts`. A region agent that needs a change
asks the coordinator. The coordinator makes one shared change at a time. It renders the
regression set and every approved spot before and after, and asks the owner.

## Rules for the shared machine

- Reach Overpass only through bake.py. It holds a lock for the whole machine.
- The coordinator serves its checkout for the owner with `npx vite --port 5180`, and slot N its
  worktree with `npx vite --port 518N`.
- Do not shoot a spot while it bakes.
- Bake again only after a bake key changes: a spot key other than `name`, or a region's `leaf`,
  `cliffs`, `summits`, `relief` or subregion areas. A look or pack change needs only new shots.
- Each texel's region paints its ground, the eye's region the sky. Choose spots whose view stays
  mostly in the region, and judge only that terrain.
- North of 65° N, judge spring and summer. The winter and autumn mosaics there are mostly empty.
- New spot ids are `<short>-<place>`. Existing ids stay as they are: bakes, shots and the source
  caches are kept per spot id.

## Data sources

- Every spot has GLO-30, WorldCover, Sentinel-2, CORINE, EGDI and IGME 5000 geology, and OSM.
- National DTMs that need no key: the `dtm` keys of the [spot table](../README.md#spots-regions-and-packs).
  Do not use sub-metre sources for the tune. They are 4 to 8 GB per spot.
- Land cover maps: `nlcm` (Ireland), `habmos` (Scotland). Geology: `igme` (Spain).
- No country- or source-specific logic in the product: a national dataset only makes the shared
  ground materials more precise at bake time, and the renderer, the shaders and the region rules
  never know which source a place has.
- A better source behind an API key is welcome if the key is free and easy to get. Agents
  cannot sign up. Write the source, its resolution, its licence and the sign-up steps into the
  tracking issue. The owner registers and puts the key in `~/.config/openbikecomputer/`.
- After the tune, the official bake pipeline takes over the best source per country.

## Demo

The demo is `index.html`:

- Pick a region, then one of its spots.
- The spot's reference photo is a small overlay in a corner. The owner can hide or show it, and
  enlarge it with a click.
- The picker shows each region's status: not started, in progress, ready for feedback, done.
  The status is one file, `~/.cache/obc/trail-view/status.json` (`{"<short>": "<status>"}`).
  Only the coordinator writes it. Every checkout serves the bake cache, so every slot's demo
  shows the same status.
- No comment boxes: feedback goes to the coordinator.

The coordinator's checkout serves the done regions on port 5180, and slot N the region it tunes
on port 518N. From the Mac:

```sh
ssh -N -L 5180:localhost:5180 -L 5181:localhost:5181 -L 5182:localhost:5182 -L 5183:localhost:5183 -L 5184:localhost:5184 box
```

Then open `http://localhost:5180/` or the slot's `http://localhost:518N/`.
