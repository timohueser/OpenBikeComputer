import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Candidate } from '../src/lib/types.ts';
import { verifyRun, verifyCatalog, reconcile, dispatch, publicationRetry } from '../src/lib/server/github.ts';
import { store } from '../src/lib/server/store.ts';
import { report } from '../src/lib/server/report.ts';
import { GET as health } from '../src/routes/health/+server.ts';

const directory = mkdtempSync(join(tmpdir(), 'obc-verification-provenance-'));
process.env.VERIFICATION_DATA_DIR = directory;
process.env.GITHUB_TOKEN = 'test-token'; process.env.GITHUB_REPOSITORY = 'test/repo';
test('dispatch IDs bind CI evidence, retries distinguish rejection from uncertainty, and reconciliation stays frozen', async () => {
  const original = globalThis.fetch;
  const c: Candidate = { id: 'candidate', version: 'v0.1.0', sourceRef: 'develop', sourceSha: 'b'.repeat(40), createdAt: '', status: 'queued', ciStatus: 'pending', revision: { id: 1, author: '', createdAt: '', requirements: [] }, results: [], manualRuns: [], assets: [] };
  const workflowSha = 'a'.repeat(40);
  const run = { id: 42, path: '.github/workflows/verification-candidate.yml', head_branch: 'develop', head_sha: workflowSha, event: 'workflow_dispatch', display_title: 'Verification candidate candidate', status: 'in_progress', conclusion: null as string | null };
  let gate = 'success';
  const payload = { runId: 42, runAttempt: 1, sourceSha: c.sourceSha, conclusion: 'success' };
  try {
    assert.equal(health().status, 200);
    globalThis.fetch = async (_url, init) => {
      assert.equal(JSON.parse(String(init?.body)).return_run_details, true);
      assert.equal(store().get<{ status: string }>('dispatch', 'candidate:verify').status, 'unknown');
      return new Response(JSON.stringify({ workflow_run_id: 42 }), { status: 200 });
    };
    await dispatch(c);
    globalThis.fetch = async (url) => new Response(JSON.stringify(String(url).includes('/jobs?') ? { total_count: 1, jobs: [{ name: 'verify', status: 'completed', conclusion: gate }] } : run), { status: 200 });
    await verifyRun(c, payload);
    // A push between source selection and dispatch does not invalidate the returned run.
    run.head_sha = 'c'.repeat(40); await verifyRun(c, payload);
    await assert.rejects(() => verifyRun(c, { ...payload, runId: 999 }), /run ID/);
    gate = 'skipped'; await assert.rejects(() => verifyRun(c, payload), /gate/);
    gate = 'success'; run.display_title = 'Verification candidate other'; await assert.rejects(() => verifyRun(c, payload), /provenance/);
    run.display_title = 'Verification candidate candidate'; await assert.rejects(() => verifyRun(c, { ...payload, sourceSha: workflowSha }), /source/);
    run.path = '.github/workflows/ci.yml'; run.event = 'push'; run.status = 'completed'; run.conclusion = 'success';
    await verifyCatalog({ runId: 42, runAttempt: 1, sourceSha: run.head_sha });
    run.event = 'pull_request'; await assert.rejects(() => verifyCatalog({ runId: 42, runAttempt: 1, sourceSha: run.head_sha }), /provenance/);
    run.path = '.github/workflows/verification-candidate.yml'; run.event = 'workflow_dispatch'; run.conclusion = 'failure';
    store().put('candidate', c.id, c);
    let calls = 0;
    globalThis.fetch = async (url) => { calls++; assert.match(String(url), /actions\/runs\/42$/); return new Response(JSON.stringify(run), { status: 200 }); };
    const failed = await reconcile(c);
    assert.equal(failed.status, 'failed'); assert.match(failed.failure!, /evidence was not received/);
    await reconcile(failed); assert.equal(calls, 1);
    const pub = { ...c, id: 'publication', status: 'publishing' as const, evidenceFrozen: true };
    store().put('candidate', pub.id, pub);
    globalThis.fetch = async () => new Response('{}', { status: 422 });
    await assert.rejects(() => dispatch(pub, true), /422/);
    assert.equal(store().get<{ status: string }>('dispatch', 'publication:publish').status, 'rejected');
    await publicationRetry(pub);
    globalThis.fetch = async () => { throw new Error('Network timeout'); };
    await assert.rejects(() => dispatch(pub, true), /Network timeout/);
    assert.equal(store().get<{ status: string }>('dispatch', 'publication:publish').status, 'unknown');
    await assert.rejects(() => publicationRetry(pub), /uncertain/);
    globalThis.fetch = async () => new Response('{}', { status: 503 });
    await assert.rejects(() => dispatch(pub, true), /503/);
    await assert.rejects(() => publicationRetry(pub), /uncertain/);
    globalThis.fetch = async () => new Response(JSON.stringify({ workflow_run_id: 43 }), { status: 200 });
    await dispatch(pub, true);
    const publishRun = { ...run, id: 43, path: '.github/workflows/verification-publish.yml', display_title: 'Publish candidate publication', status: 'in_progress', conclusion: null as string | null };
    globalThis.fetch = async () => new Response(JSON.stringify(publishRun), { status: 200 });
    await assert.rejects(() => publicationRetry(pub), /still running/);
    publishRun.status = 'completed'; publishRun.conclusion = 'failure';
    await publicationRetry(pub);
    const interrupted = await reconcile(pub);
    assert.equal(interrupted.status, 'publishing'); assert.equal(interrupted.evidenceFrozen, true); assert.match(interrupted.failure!, /confirmation was not recorded/);
    publishRun.conclusion = 'success'; await assert.rejects(() => publicationRetry(pub), /passed/);
  } finally {
    globalThis.fetch = original;
    store().db.close(); assert.equal(health().status, 503);
    rmSync(directory, { recursive: true, force: true });
  }
});
test('standalone reports render Markdown without active content', () => {
  const c: Candidate = { id: 'candidate', version: '<img src=x onerror=alert(1)>', sourceRef: 'develop', sourceSha: 'a'.repeat(40), createdAt: '', status: 'queued', ciStatus: 'pending', revision: { id: 1, author: '', createdAt: '', requirements: [{ id: 'REQ-1', title: '<script>bad()</script>', group: 'Navigation <script>bad()</script>', statement: '**Bold** <script>bad()</script> [bad](javascript:bad())', active: true, tests: [] }] }, results: [], manualRuns: [], assets: [] };
  const html = report(c);
  assert.match(html, /<strong>Bold<\/strong>/);
  assert.match(html, /Group: Navigation &lt;script&gt;/);
  assert.doesNotMatch(html, /<script>|href="javascript:|<img/);
});

test('reports distinguish accepted gaps from passes and retain unfinished definitions and original failures', () => {
  const c: Candidate = { id: 'exceptions', version: 'v0.1.0-alpha', sourceRef: 'develop', sourceSha: 'a'.repeat(40), createdAt: '', status: 'ready', ciStatus: 'success',
    revision: { id: 1, author: 'owner', createdAt: '', requirements: [{ id: 'REQ-1', title: 'Runtime', statement: 'Defined target', active: true, tests: [{ id: 'manual', title: 'Battery run', kind: 'manual', steps: 'Measure', expected: 'Meets target', inputs: [] }] }] },
    results: [], manualRuns: [{ id: 'run', requirementId: 'REQ-1', testId: 'manual', result: 'fail', device: 'alpha board', notes: 'Measured below target', evidence: [], author: 'tester', createdAt: '' }],
    assets: ['UPDATE.BIN', 'manifest.json', 'SHA256SUMS.txt', 'obc-boot.elf', 'obc-fw-nrf54l.elf'].map(name => ({ id: name, name, size: 1, sha256: 'a'.repeat(64) })),
    exceptions: [{ requirementId: 'REQ-1', reason: 'Alpha limitation <script>bad()</script>', author: 'admin', createdAt: '2026-09-16' }] };
  const html = report(c);
  assert.match(html, /<h2>Accepted with exceptions<\/h2>/);
  assert.match(html, /0\/1 requirements verified · 1 accepted with exception/);
  assert.match(html, /<strong>fail<\/strong>/);
  assert.match(html, /Alpha limitation &lt;script&gt;/);
  assert.match(html, /Accepted by admin/);
  assert.doesNotMatch(html, /<script>|<h2>Verified<\/h2>/);
  c.revision.requirements[0].todo = true;
  assert.match(report(c), /<h2>Incomplete<\/h2>/);
  assert.match(report(c), /Definition incomplete — requirement definition needs work/);
  c.revision.requirements[0].todo = false;
  c.revision.requirements.push({ id: 'REQ-2', title: 'Future <feature>', statement: 'Target to define', active: false, todo: true, implementationNeeded: true, tests: [] });
  const excluded = report(c);
  assert.match(excluded, /<h2>Accepted with exceptions and exclusions<\/h2>/);
  assert.match(excluded, /1 accepted with exception · 1 excluded/);
  assert.match(excluded, /<h2>Excluded requirements<\/h2>/);
  assert.match(excluded, /REQ-2 — Future &lt;feature&gt;/);
  assert.match(excluded, /Implementation needed/);
  assert(excluded.indexOf('<h2>Excluded requirements</h2>') < excluded.indexOf('<article>'));
  assert.doesNotMatch(excluded, /REQ-2: no verification defined/);
  c.exceptions = [];
  c.manualRuns[0].result = 'pass';
  assert.match(report(c), /Coverage: Not reviewed/);
  c.revision.requirements[0].coverage = { sourceSha: c.sourceSha, conclusion: 'complete', rationale: 'The manual check measures the required target.', criteria: [{ id: 'runtime', statement: 'Meets the runtime target.', evidence: [{ testId: 'manual', rationale: 'Measures runtime on the board.' }], gap: '' }], review: { author: 'owner', createdAt: '', proposalId: 'review' } };
  assert.match(report(c), /<h2>Accepted with exclusions<\/h2>/);
});
