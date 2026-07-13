TR3 (#652) — trip folder-row visual variants for the owner to pick.

Flip `FOLDER_ROW_STYLE` in firmware/obc-app/src/screen/route_menu.rs to A / B / C.
Variant A is the active default. All three share the full trip machinery — only
draw_folder_row() branches on the const. These PNGs are NOT committed.

variant-a.png  (ACTIVE)  Folder glyph before the name + summed distance right-aligned on
                         line 1; "N routes" on the left of line 2, the summed climb at the
                         route-row climb column. Shows all four figures; a long name truncates.

variant-b.png            No glyph — the full trip name gets line 1 to itself (never crowded),
                         with a single "N routes · KM km" caption on line 2. The most legible,
                         name-first look; its tradeoff is it drops the summed climb.

variant-c.png            A drawn folder-tab icon + a rounded count badge on the name line, with
                         the summed km / climb on line 2 in the SAME two columns as a route row,
                         so the stats align down the list. Folderness carried by icon + badge.

Other TR3 screens (rendered with variant A, the active default):

stage-list.png            Drilling into the "Alpen Traverse" folder: its member routes as
                          standard route rows, under the trip's name as the title.

trip-delete-confirm.png   Long-press the folder → the cascade-delete confirm: warning-red
                          hold-guarded "Delete all" + "Cancel", naming the trip. Entry rests
                          on Cancel (nothing armed on the way in).
