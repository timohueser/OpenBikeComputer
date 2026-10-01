CREATE INDEX address_lookup ON addresses(street_id,house);
CREATE INDEX address_cells ON addresses(
 CAST((lat+90)*200 AS INTEGER)*72001+CAST((lon+180)*200 AS INTEGER));
CREATE INDEX category_lookup ON place_records(kind,lon,lat);
CREATE VIRTUAL TABLE spatial USING rtree(id,west,east,south,north);
INSERT INTO spatial SELECT id,lon,lon,lat,lat FROM places;
