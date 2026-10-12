# obc data wireframes

The owner-approved wireframes of the operator page for epic #2648. Issue #2653 describes every
screen in words; these files show the layout. The data in them is a fake scenario, not
measurements.

## View

```sh
python3 -m http.server 8765 --directory wireframes/obc-data
```

Open `http://127.0.0.1:8765/`. Screens are clickable: target switch, layer cards, review,
rollback, lock takeover, area tabs and Stop all work. `png/` holds screenshots of each screen and of
its main clicked states.

## Files

| Path | Content |
| --- | --- |
| `screens/*.dc.html` | One screen each: markup, plus the scenario data and handlers in its script |
| `screens/canvas.json` | Screen titles and the flow rows (F1 to F8) |
| `render.html` | Renders one screen; supports only the template features these screens use |
| `index.html` | Screen list beside the rendered screen |
| `png/` | Screenshots at 1440 px width |

The scenario:
- `prod` covers DACH + Baden-Württemberg; its release is 3 days old.
- 6 layers are stale: a `device/map` revision change, a new `wiki` version, and the layers that follow them.
- `local` covers Freiburg.
- The laptop has a charcoal header; the bake machine `hetzner-bake-1` has a purple one.
