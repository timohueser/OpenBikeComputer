import {test} from 'node:test';
import assert from 'node:assert/strict';
import {reverseAddress} from '../web/reverse.mjs';
import {database} from './database.mjs';
import {distance} from '../web/engine.mjs';

test('reverse naming uses the nearest house, including houses far from the street centre', () => {
  const {db,conn} = database();
  assert.equal(reverseAddress(db,[7.8511,47.991]),'Kaiser-Joseph-Straße 12, Freiburg');
  assert.equal(reverseAddress(db,[7.854,48.0301]),'Habsburgerstraße 10, Freiburg');
  assert.equal(reverseAddress(db,[7.86,48.03]),null);
  for (const coordinate of [null,[7.8],[NaN,48],[181,48],[7.8,91]])
    assert.throws(() => reverseAddress(db,coordinate));
  conn.close();
});

test('address cells preserve nearest houses across grid edges and signed coordinates', () => {
  const {db,conn} = database();
  conn.exec('DELETE FROM addresses');
  const insert = conn.prepare('INSERT INTO addresses VALUES (10,?,?,?,?)');
  for (const [x,y] of [[7.85,48.03],[-7.85,-48.03],[0,0],[179.999,80],[-179.999,-80]])
    for (const dx of [-.0003,0,.0003]) for (const dy of [-.0003,0,.0003])
      insert.run(`${x},${y},${dx},${dy}`,x+dx,y+dy,`n${x},${y},${dx},${dy}`);
  const addresses = db.all('SELECT * FROM addresses');
  for (const address of addresses) for (const offset of [[0,0],[-.0002,.0002],[.0002,-.0002]]) {
    const point = [address.lon+offset[0],address.lat+offset[1]];
    const nearest = addresses.map(a=>({...a,distance:distance(point,[a.lon,a.lat])}))
      .filter(a=>a.distance<=.1).sort((a,b)=>a.distance-b.distance||a.source.localeCompare(b.source))[0];
    assert.equal(reverseAddress(db,point),nearest?`Kaiser-Joseph-Straße ${nearest.house}, Freiburg`:null);
  }
  conn.close();
});
