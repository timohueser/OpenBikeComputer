import {around, distance} from './engine.mjs';

export function reverseAddress(db, coordinate) {
  if (!Array.isArray(coordinate) || coordinate.length !== 2 || !coordinate.every(Number.isFinite)
      || Math.abs(coordinate[0]) > 180 || Math.abs(coordinate[1]) > 90)
    throw new Error('Provide a longitude and latitude.');
  // A distant address must not name an isolated point.
  const radius = .1;
  const [west, south, east, north] = around(coordinate, radius);
  const candidates = db.all(`SELECT a.house,a.lon,a.lat,p.name,p.city,a.source
    FROM address_spatial b CROSS JOIN addresses a ON a.rowid=b.id
    JOIN places p ON p.id=a.street_id
    WHERE b.east>=? AND b.north>=? AND b.west<=? AND b.south<=?`, [west,south,east,north]);
  const nearest = candidates.map(address => ({...address, distance: distance(coordinate,[address.lon,address.lat])}))
    .filter(address => address.distance <= radius)
    .sort((a,b) => a.distance-b.distance || a.source.localeCompare(b.source))[0];
  return nearest ? `${nearest.name} ${nearest.house}${nearest.city ? `, ${nearest.city}` : ''}` : null;
}
