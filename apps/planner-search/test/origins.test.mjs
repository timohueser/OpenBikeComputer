import { test } from 'node:test';
import assert from 'node:assert/strict';
import { allowedOrigin } from '../origins.mjs';

test('public search accepts only configured browser origins', () => {
  const origins = new Set(['https://openbikecomputer.com']);
  for (const origin of [undefined, 'https://openbikecomputer.com']) assert.equal(allowedOrigin(origin, origins), true);
  for (const origin of ['null', 'garbage', 'https://openbikecomputer.com.evil.test', 'https://openbikecomputer.com/path', 'http://localhost:4175']) assert.equal(allowedOrigin(origin, origins), false);
});
test('local previews allow loopback origins without a hosted configuration', () => {
  for (const origin of ['http://localhost:4175', 'http://127.0.0.1:4175', 'http://[::1]:4175']) assert.equal(allowedOrigin(origin, new Set()), true);
  assert.equal(allowedOrigin('https://example.com', new Set()), false);
});
