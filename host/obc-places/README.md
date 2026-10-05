# Shared OSM places

This host crate defines POI tag priority and area coordinates for the device and
planner bakers. Device subtype IDs and labels come from `obc-formats`.

Run `obc test -p obc-places -p obc-pack -p obc-search-bake` from the checkout.
The search baker's integration suite compares both producers on the same OSM input.
