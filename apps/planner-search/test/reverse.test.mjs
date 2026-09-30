import {test} from 'node:test';
import assert from 'node:assert/strict';
import {reverseAddress} from '../web/reverse.mjs';
import {database} from './database.mjs';

test('reverse naming uses the nearest house, including houses far from the street centre', () => {
  const {db,conn} = database();
  assert.equal(reverseAddress(db,[7.8511,47.991]),'Kaiser-Joseph-Straße 12, Freiburg');
  assert.equal(reverseAddress(db,[7.854,48.0301]),'Habsburgerstraße 10, Freiburg');
  assert.equal(reverseAddress(db,[7.86,48.03]),null);
  for (const coordinate of [null,[7.8],[NaN,48],[181,48],[7.8,91]])
    assert.throws(() => reverseAddress(db,coordinate));
  conn.close();
});
