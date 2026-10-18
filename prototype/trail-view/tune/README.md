# Europe tune

The tune gives each baseline ecoregion of Europe its rules and assets, so that Trail View looks
right everywhere. The baseline is the 45 regions in `tune/ecoregions.json` (RESOLVE Ecoregions
2017, Europe to 60° E, without the regions only in Russia). Each region has a short name, such as
`alps` or `celtic`.

One tracking issue on GitHub holds the batch plan, the asset census and the status of each region.
This file holds only the instructions.

## Start

On the Linux box, finish the setup in [../README.md](../README.md#linux-setup). Then start one
Claude Code session in the coordinator worktree and give it this message:

> Read prototype/trail-view/tune/README.md and act as the coordinator of the Europe tune.

Run bake.py from the repository root. Run every other command in `prototype/trail-view`.

## Roles

- **Owner.** Judges each region on the review page, writes comments and approves. Registers API
  keys.
- **Coordinator.** One session in its own worktree on `archive/trail-view-prototype`. It runs
  phase 1 and the census, plans the batches, starts the region agents, merges their branches,
  owns the frozen files and renders the regression set after each shared change.
- **Region agent.** One session per region (Opus), in its own worktree on branch `tune/<short>`.
  It writes only `regions/<short>.json`, `regions/<short>.*.json`, `packs/<short>.json`,
  `packs/<short>.*.json`, `spots/<short>.json` and the asset files that the census gave it.
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
   ramps (one per season where needed), the barren rule, tree elevation bands, the snowline,
   `windswept_m`, `turf_deg`, and culture by country. Split only where a key that applies at the
   eye (haze, weather, grade, ramps) must change across stable geography, such as an island side,
   a massif or a coast strip. Never draw contour rings. Ask the coordinator for a height rule
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
one alpine and one Mediterranean region, so that the four share few assets.

Each region agent repeats these steps until the owner approves:

1. **Bake.** `uv run prototype/trail-view/bake.py <spot>` for each spot. Give a spot the best
   data that needs no key with its `dtm`, `landcover` and `geology` keys. See "Data sources".
2. **Tune.** Change the region's look and rules and the pack. `npm run check` must pass.
3. **Shoot.** `npm run shoot -- <spot>,<spot> --round <n>`, with `--swiftshader` on a machine
   without a GPU. This writes the round to the review page. Look at the PNGs with the Read tool
   before the owner does.
4. **Post.** `node tune/comment.mjs <short> --round <n> "<what changed and why>"`.
5. **Read.** The owner comments on `review.html`. The comments are in
   `~/.cache/obc/trail-view/review/<short>/comments.jsonl`, one JSON object per line with its
   `round`, `spot`, `by` and `approve`. Start the next round.
6. **Land.** After the approval: `npm run check`, then one commit on `tune/<short>`. The
   coordinator rebases it onto `archive/trail-view-prototype` and fast-forwards.

## Claims: each asset is built once

An asset is a tree, plant or animal model (`src/models/trees/`, `plants/` or `animals/<id>.ts`), a
house style (`styles/<id>.json`) or a building kind (`src/models/buildings.ts`). Its id is the
English name as a slug, for example `pedunculate-oak`. Its file holds the Latin name. One Latin
name has one model; `npm run check` fails on two.

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
`src/shaders.ts`, `src/terrain.ts`, `src/nearfield.ts`, `src/palette.ts`, `src/light.ts`,
`src/weather.ts`, `src/regions.ts` or `src/models/shape.ts`. A region agent that needs a change
asks the coordinator. The coordinator makes one shared change at a time. It renders the
regression set and every approved spot before and after, and asks the owner.

## Rules for the shared machine

- Reach Overpass only through bake.py. It holds a lock for the whole machine.
- Do not run `npm run dev`: its port 5180 is strict. Slot N serves its worktree for the owner with
  `npx vite --port 518N`.
- Do not shoot a spot while it bakes.
- Bake again only after a bake key changes: a spot key other than `name`, or a region's `leaf`,
  `cliffs`, `summits`, `relief` or subregion areas. A look or pack change needs only new shots.
- The region at the eye paints the whole panorama. Choose spots whose view stays mostly in the
  region, and judge only that terrain.
- North of 65° N, judge spring and summer. The winter and autumn mosaics there are mostly empty.
- Spot ids are `<short>-<place>`. Bakes and shots are kept per spot id.

## Data sources

- Every spot has GLO-30, ESA WorldCover, Sentinel-2 colour and OpenStreetMap.
- National DTMs that need no key, at 1 to 5 m: `ch`, `de-bw` and the other `de-*`, `at`, `uk`
  (England), `no`, `fr`, `es`, `scot`, `it-bz`. Do not use sub-metre sources for the tune. They are
  4 to 8 GB per spot.
- Land cover maps: `nlcm` (Ireland), `habmos` (Scotland). Geology: `igme` (Spain).
- A better source behind an API key is welcome if the key is free and easy to get. Agents
  cannot sign up. Write the source, its resolution, its licence and the sign-up steps into the
  tracking issue. The owner registers and puts the key in `~/.config/openbikecomputer/`.
- After the tune, the official bake pipeline takes over the best source per country.

## Review page

Each slot serves `review.html`. The owner opens it from the Mac through an SSH tunnel:
`ssh -N -L 518N:localhost:518N box`, then `http://localhost:518N/review.html`. The page lists
the regions with their status, the spots with their reference photos, each round beside the
previous one, links to the live view, and a comment box per region and per spot. A comment or an
approval belongs to the round on the screen.
