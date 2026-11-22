---
title: How TrailView models look
description: The one style of every TrailView model, and why we make each model in code with one kit.
---

# How TrailView models look

TrailView draws many small models: animals, trees, plants, rocks, benches and other objects. All of
them have one style, "soft low-poly". This page says what the style is and why we keep it. The bytes
of a model file and the triangle budget of each kind are in the
[model contract](src:specs/trailview-models.md).

![A row of the herd animals on a meadow in TrailView: cattle, sheep, goats, a horse, a donkey, a pig, a dromedary, deer, reindeer, ibex and chamois](../../assets/trailview-models/lineup.jpg)

## The style

A model is low in detail and looks like a model from a game. It is friendly and never odd. A rider
sees it for a moment, at the side of a trail, so its shape must read at once.

- **Round, faceted shapes.** A body is a skin through rings of 8 to 12 sides. Each face has one flat
  colour and one normal, so the facets show and the light gives the shape.
- **Clear outlines.** A model reads from the side and from above. Legs, ears, horns and tails stand
  out from the body. A detail that a rider cannot see at 10 m does not get a triangle.
- **Friendly proportions.** An animal has a slightly large head with eyes and a pale muzzle, short
  sturdy legs and a round body. The proportions come from the real species; the style only rounds
  and softens them.
- **Few colours.** A coat has two or three colours in large areas: a back, a belly, a face, patches
  with clear edges. Many small colour changes look like noise at a distance.

## Why we make every model in code

We make all models ourselves, in TypeScript, with one shared kit
([`sculpt.ts`](src:builder/web/src/lib/trailview/models/sculpt.ts)). We do not ship models from
asset packs or from image-to-3-D tools, even with a free licence. A model from another source has
another style, and two sources never agree. Packs and photos are only references.

The kit has few parts:

- a **skin** through rings, with a separate reach above and below the middle, a pitch and a
  squareness, for bodies, necks, heads and legs;
- a **leaf**, a flat diamond, for ears, beards and short tails;
- a **chain** of bars for horns, antlers and tails.

Each face of a part turns away from a point inside the part, so no model needs a hand-made winding
order. The animals share one more layer, a four-legged body
([`quadruped.ts`](src:builder/web/src/lib/trailview/models/animals/body/quadruped.ts)) that a
breed fills with its numbers.

## Tiers: one build, two levels of detail

Each model builds its near tier and its far tier from the same code. The far tier uses fewer sides
and leaves out fine details such as eyes and udders. So the outline and the colour areas stay the
same when an animal moves from one tier to the other. For an animal, the near tier ends at 150 m.
Past it, an animal is only a few pixels tall.

![The cattle of TrailView in the near tier (left) and the far tier (right), drawn at the same size](../../assets/trailview-models/tiers.jpg)

## Poses and coats

An animal has two or three poses: it stands, it grazes, or it lies. A herd mixes them, so a group
looks alive. A breed has one to four coats, and most animals of a group wear the first coat.

![A brown cow standing, grazing and lying](../../assets/trailview-models/poses.jpg)

## A new model

A new model or a change to a model is a change of look. The review page
(`builder/web/tests/browser/trailview/models.mjs`) shows each model in TrailView's own light, near
and far, with its triangle and vertex counts against the budget, and the old model beside it. The
owner approves the pictures before the change merges.
