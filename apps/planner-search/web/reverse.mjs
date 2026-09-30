import {around, distance} from './engine.mjs';

export function reverseAddress(db, coordinate) {
  if (!Array.isArray(coordinate) || coordinate.length !== 2 || !coordinate.every(Number.isFinite)
      || Math.abs(coordinate[0]) > 180 || Math.abs(coordinate[1]) > 90)
    throw new Error('Provide a longitude and latitude.');
  // A distant address must not name an isolated point.
  const radius = .1;
  const [west, south, east, north] = around(coordinate, radius);
  // Match the expression index in indexes.sql. Original doubles decide the distance.
  const cellX = lon => Math.trunc((Math.max(-180,Math.min(180,lon))+180)*200);
  const cellY = lat => Math.trunc((Math.max(-90,Math.min(90,lat))+90)*200);
  const candidates = [];
  for (let y=cellY(south); y<=cellY(north); y++) {
    candidates.push(...db.all(`SELECT a.house,a.lon,a.lat,p.name,p.city,a.source
      FROM addresses a JOIN places p ON p.id=a.street_id
      WHERE CAST((a.lat+90)*200 AS INTEGER)*72001+CAST((a.lon+180)*200 AS INTEGER) BETWEEN ? AND ?`,
      [y*72001+cellX(west),y*72001+cellX(east)]));
  }
  const nearest = candidates.map(address => ({...address, distance: distance(coordinate,[address.lon,address.lat])}))
    .filter(address => address.distance <= radius)
    .sort((a,b) => a.distance-b.distance || a.source.localeCompare(b.source))[0];
  return nearest ? `${nearest.name} ${nearest.house}${nearest.city ? `, ${nearest.city}` : ''}` : null;
}
