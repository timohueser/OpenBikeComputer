import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Candidate } from '../src/lib/types.ts';
import { verifyRun, verifyCatalog, reconcile } from '../src/lib/server/github.ts';
import { store } from '../src/lib/server/store.ts';
import { report } from '../src/lib/server/report.ts';

const directory = mkdtempSync(join(tmpdir(), 'obc-verification-provenance-'));
process.env.VERIFICATION_DATA_DIR = directory;
process.env.GITHUB_TOKEN = 'test-token'; process.env.GITHUB_REPOSITORY = 'test/repo';
test('CI evidence binds trusted workflow revision, candidate identity and terminal gate', async () => {
  const original = globalThis.fetch;
  const c: Candidate = { id: 'candidate', version: '0.1.0', sourceRef: 'develop', sourceSha: 'b'.repeat(40), createdAt: '', status: 'queued', ciStatus: 'pending', revision: { id: 1, author: '', createdAt: '', requirements: [] }, results: [], manualRuns: [], assets: [] };
  const workflowSha = 'a'.repeat(40);
  store().put('dispatch', 'candidate:verify', { workflowSha });
  const run = { path: '.github/workflows/verification-candidate.yml', head_branch: 'develop', head_sha: workflowSha, event: 'workflow_dispatch', display_title: 'Verification candidate candidate' };
  let gate = 'success';
  globalThis.fetch = async (url) => new Response(JSON.stringify(String(url).includes('/jobs?') ? { total_count: 1, jobs: [{ name: 'verify', status: 'completed', conclusion: gate }] } : run), { status: 200, headers: { 'content-type': 'application/json' } });
  const payload = { runId: 42, runAttempt: 1, sourceSha: c.sourceSha, conclusion: 'success' };
  try {
    await verifyRun(c, payload);
    gate = 'skipped'; await assert.rejects(() => verifyRun(c, payload), /gate/);
    gate = 'success'; run.head_sha = c.sourceSha; await assert.rejects(() => verifyRun(c, payload), /provenance/);
    run.head_sha = workflowSha; run.display_title = 'Verification candidate other'; await assert.rejects(() => verifyRun(c, payload), /provenance/);
    run.display_title = 'Verification candidate candidate'; await assert.rejects(() => verifyRun(c, { ...payload, sourceSha: workflowSha }), /source/);
    run.path = '.github/workflows/ci.yml'; run.event = 'push'; Object.assign(run, { status: 'completed', conclusion: 'success' });
    await verifyCatalog({ runId: 42, runAttempt: 1, sourceSha: workflowSha });
    run.event = 'pull_request'; await assert.rejects(() => verifyCatalog({ runId: 42, runAttempt: 1, sourceSha: workflowSha }), /provenance/);
    store().put('candidate', c.id, c);
    const at = new Date().toISOString();
    store().put('dispatch', 'candidate:verify', { workflowSha, at });
    let calls = 0;
    globalThis.fetch = async () => { calls++; return new Response(JSON.stringify({ workflow_runs: [{ display_title: 'Verification candidate candidate', head_sha: workflowSha, created_at: at, status: 'completed', conclusion: 'failure', id: 42 }] }), { status: 200 }); };
    const failed = await reconcile(c);
    assert.equal(failed.status, 'failed'); assert.match(failed.failure!, /evidence was not received/);
    await reconcile(failed); assert.equal(calls, 1);
    const pub = { ...c, id: 'publication', status: 'publishing' as const, evidenceFrozen: true };
    store().put('candidate', pub.id, pub); store().put('dispatch', 'publication:publish', { workflowSha, at });
    globalThis.fetch = async () => new Response(JSON.stringify({ workflow_runs: [{ display_title: 'Publish candidate publication', head_sha: workflowSha, created_at: at, status: 'completed', conclusion: 'failure', id: 43 }] }), { status: 200 });
    const interrupted = await reconcile(pub);
    assert.equal(interrupted.status, 'publishing'); assert.equal(interrupted.evidenceFrozen, true); assert.match(interrupted.failure!, /confirmation was not recorded/);

  } finally { globalThis.fetch = original; store().db.close(); rmSync(directory, { recursive: true, force: true }); }
});
test('standalone reports render Markdown without active content', () => {
  const c: Candidate = { id: 'candidate', version: '<img src=x onerror=alert(1)>', sourceRef: 'develop', sourceSha: 'a'.repeat(40), createdAt: '', status: 'queued', ciStatus: 'pending', revision: { id: 1, author: '', createdAt: '', requirements: [{ id: 'REQ-1', title: '<script>bad()</script>', statement: '**Bold** <script>bad()</script> [bad](javascript:bad())', active: true, tests: [] }] }, results: [], manualRuns: [], assets: [] };
  const html = report(c);
  assert.match(html, /<strong>Bold<\/strong>/);
  assert.doesNotMatch(html, /<script>|href="javascript:|<img/);
});
