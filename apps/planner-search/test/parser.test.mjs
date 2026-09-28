import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { parserProcess } from '../parser.mjs';

test('timed-out requests keep their worker slots until completion', async (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'planner-worker-'));
  const executable = join(directory, 'worker');
  writeFileSync(executable, `#!${process.execPath}\nconsole.log(JSON.stringify({ready:true})); process.stdin.resume();\n`, {mode:0o755});
  const parser = parserProcess(executable, directory);
  t.after(() => { parser.close(); rmSync(directory, {recursive:true,force:true}); });
  for (let i = 0; i < 100 && !parser.status().ready; i++) await delay(20);
  assert.equal(parser.status().ready, true);
  t.mock.timers.enable({apis:['setTimeout']});
  const requests = Array.from({length:8}, () => assert.rejects(parser.parse('hotels'), /timed out/));
  t.mock.timers.tick(10_000);
  await Promise.all(requests);
  await assert.rejects(parser.parse('hotels'), /busy/);
});
