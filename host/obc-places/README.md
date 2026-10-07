# Shared OSM places

This host crate defines POI tag priority, names, schedules, approach metadata and
routable source records for the device and planner bakers. Device subtype IDs and
labels come from `obc-formats`. Graph construction stays in the network producer.

Run `obc test -p obc-places -p obc-bake -p obc-search-bake` from the checkout.
The search baker's integration suite compares both producers on the same OSM input.
