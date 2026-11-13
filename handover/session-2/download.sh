#!/usr/bin/env bash
set -uo pipefail
cd /home/user/obc-bake/switzerland
curl -fsSL --retry 4 -o switzerland-latest.osm.pbf https://download.openstreetmap.fr/extracts/europe/switzerland-latest.osm.pbf
for lat in 45 46 47; do for lon in 005 006 007 008 009 010; do
  S=Copernicus_DSM_COG_10_N${lat}_00_E${lon}_00_DEM
  curl -fsSL --retry 4 -o glo30/$S.tif https://copernicus-dem-30m.s3.amazonaws.com/$S/$S.tif || echo "FAILED $S"
done; done
ls -la . glo30 | head -40; du -sh .
echo downloads-done
