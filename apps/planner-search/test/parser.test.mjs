import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { parserProcess } from '../parser.mjs';

async function start(t, script) {
  const directory = mkdtempSync(join(tmpdir(), 'planner-worker-'));
  const executable = join(directory, 'worker');
  writeFileSync(executable, `#!${process.execPath}\nconsole.log(JSON.stringify({ready:true}));\n${script}\n`, {mode:0o755});
  const failures = [];
  const parser = parserProcess(executable, directory, (message) => failures.push(message));
  t.after(() => { parser.close(); rmSync(directory, {recursive:true,force:true}); });
  for (let i = 0; i < 100 && !parser.status().ready; i++) await delay(20);
  assert.equal(parser.status().ready, true);
  return {parser, failures};
}

test('a hung runtime fails every pending request and reports one failure', async (t) => {
  const {parser, failures} = await start(t, 'process.stdin.resume();');
  t.mock.timers.enable({apis:['setTimeout']});
  const requests = Array.from({length:3}, () => assert.rejects(parser.parse('hotels'), /timed out/));
  t.mock.timers.tick(10_000);
  await Promise.all(requests);
  assert.deepEqual(failures, ['The query model timed out. Try again.']);
  await assert.rejects(parser.parse('hotels'), /timed out/);
});

test('a stopped runtime fails its request and reports one failure', async (t) => {
  const {parser, failures} = await start(t, 'process.stdin.once("data", () => process.exit(3));');
  await assert.rejects(parser.parse('hotels'), /stopped/);
  assert.deepEqual(failures, ['The query runtime stopped.']);
});

test('closing fails pending and later requests without a failure report', async (t) => {
  const {parser, failures} = await start(t, 'process.stdin.resume();');
  const waiting = parser.parse('hotels');
  parser.close();
  await assert.rejects(waiting, /stopped/);
  await assert.rejects(parser.parse('hotels'), /stopped/);
  assert.equal(parser.status().ready, false);
  assert.deepEqual(failures, []);
});

test('a reply that waits while the thread is busy beats the deadline', async (t) => {
  const {parser, failures} = await start(t,
    'require("readline").createInterface({input:process.stdin}).on("line",l=>console.log(JSON.stringify({id:JSON.parse(l).id,result:"ok"})));');
  t.mock.timers.enable({apis:['setTimeout']});
  const reply = parser.parse('hotels');
  // Block the thread until the reply waits in the pipe, then pass the deadline.
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1000);
  t.mock.timers.tick(10_000);
  assert.equal(await reply, 'ok');
  assert.deepEqual(failures, []);
});
