import { assert, positive, text } from './domain.ts';
import { store } from './store.ts';
import type { Candidate } from '../types.ts';

export const githubEnabled = () => !!(process.env.GITHUB_TOKEN && process.env.GITHUB_REPOSITORY);
export const allowedBranch = () => process.env.VERIFICATION_SOURCE_BRANCH || 'develop';
export async function github(path: string, body?: unknown): Promise<any> {
  assert(githubEnabled(), 'GitHub integration is not configured.', 503);
  const response = await fetch(`https://api.github.com/repos/${process.env.GITHUB_REPOSITORY}/${path}`, {
    method: body === undefined ? 'GET' : 'POST', headers: { Authorization: `Bearer ${process.env.GITHUB_TOKEN}`, Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
    body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(30000)
  });
  assert(response.ok, `GitHub request failed (${response.status}).`, 502);
  return response.status === 204 ? undefined : response.json();
}
export async function sourceCommit(ref: string): Promise<{ sourceSha: string; workflowSha: string }> {
  const branch = allowedBranch();
  assert(ref === branch || /^[a-f0-9]{40}$/.test(ref), `Choose ${branch} or an exact commit from that branch.`);
  const head = await github(`commits/${encodeURIComponent(branch)}`);
  const source = ref === branch ? head : await github(`commits/${ref}`);
  if (source.sha !== head.sha) {
    const comparison = await github(`compare/${source.sha}...${head.sha}`);
    assert(comparison.status === 'ahead' || comparison.status === 'identical', `Source commit must be part of ${branch}.`);
  }
  return { sourceSha: source.sha, workflowSha: head.sha };
}
export async function dispatch(candidate: Candidate, publish = false): Promise<void> {
  const workflow = publish ? 'verification-publish.yml' : 'verification-candidate.yml';
  const commit = await github(`commits/${encodeURIComponent(allowedBranch())}`);
  store().put('dispatch', `${candidate.id}:${publish ? 'publish' : 'verify'}`, { workflowSha: commit.sha, at: new Date().toISOString() });
  await github(`actions/workflows/${workflow}/dispatches`, { ref: allowedBranch(), inputs: publish ? { candidate_id: candidate.id } : { candidate_id: candidate.id, source_sha: candidate.sourceSha, release_version: candidate.version } });
}
export async function verifyRun(candidate: Candidate, body: Record<string, any>, publish = false): Promise<void> {
  assert(body.sourceSha === candidate.sourceSha, 'Candidate source does not match.', 409);
  const runId = positive(body.runId, 'Run ID');
  const attempt = positive(body.runAttempt, 'Run attempt');
  const run = await github(`actions/runs/${runId}/attempts/${attempt}`);
  const expected = store().get<{ workflowSha: string }>('dispatch', `${candidate.id}:${publish ? 'publish' : 'verify'}`);
  const path = `.github/workflows/${publish ? 'verification-publish' : 'verification-candidate'}.yml`;
  assert(run.event === 'workflow_dispatch' && run.path === path && run.head_branch === allowedBranch() && run.head_sha === expected.workflowSha && run.display_title === `${publish ? 'Publish' : 'Verification'} candidate ${candidate.id}`, 'Workflow provenance does not match the candidate.', 403);
  if (publish || body.conclusion === 'success') {
    let jobs: any[] = [];
    for (let page = 1; page <= 20; page++) {
      const result = await github(`actions/runs/${runId}/attempts/${attempt}/jobs?per_page=100&page=${page}`);
      jobs.push(...result.jobs);
      if (jobs.length >= result.total_count) break;
      assert(page < 20, 'Too many workflow jobs.');
    }
    const gate = jobs.filter((j) => j.name === (publish ? 'publish' : 'verify'));
    assert(gate.length === 1 && gate[0].status === 'completed' && gate[0].conclusion === 'success', 'Required workflow gate has not passed.', 409);
  }
  if (!publish && candidate.runId) assert(candidate.runId === runId && candidate.runAttempt === attempt, 'This candidate already has results from a different execution. Prepare a new candidate.', 409);
}
export async function verifyPublished(candidate: Candidate, body: Record<string, any>): Promise<string> {
  await verifyRun(candidate, body, true);
  const release = await github(`releases/tags/${encodeURIComponent(candidate.version)}`);
  assert(!release.draft && release.html_url === text(body.releaseUrl, 'Release URL', 2000), 'Published release was not found.');
  const tag = await github(`commits/${encodeURIComponent(candidate.version)}`);
  assert(tag.sha === candidate.sourceSha, 'Release tag does not match candidate.');
  for (const expected of candidate.assets) {
    const found = release.assets.find((a: any) => a.name === expected.name);
    assert(found && found.size === expected.size && found.digest === `sha256:${expected.sha256}`, `Published asset does not match: ${expected.name}`);
  }
  return release.html_url;
}

export async function publicationRetry(candidate: Candidate): Promise<void> {
  const runs = await github('actions/workflows/verification-publish.yml/runs?event=workflow_dispatch&per_page=100');
  const matching = runs.workflow_runs.filter((run: any) => run.display_title === `Publish candidate ${candidate.id}`);
  assert(matching.length > 0, 'Publication is starting or its dispatch outcome is uncertain. Check GitHub before retrying.', 409);
  assert(matching.every((run: any) => run.status === 'completed'), 'Publication is still running.', 409);
  assert(matching.every((run: any) => ['failure', 'cancelled', 'timed_out', 'action_required'].includes(run.conclusion)), 'A publication workflow passed. Reconcile its result before retrying.', 409);
}
export async function verifyCatalog(data: Record<string, any>): Promise<void> {
  const runId = positive(data.runId, 'Catalogue run ID');
  const attempt = positive(data.runAttempt, 'Catalogue run attempt');
  const run = await github(`actions/runs/${runId}/attempts/${attempt}`);
  assert(run.path === '.github/workflows/ci.yml' && run.head_branch === allowedBranch() && run.head_sha === data.sourceSha && ['push', 'workflow_dispatch'].includes(run.event) && run.status === 'completed' && run.conclusion === 'success', 'Catalogue provenance must be successful CI for the allowed branch.', 403);
}

const reconciled = new Map<string, number>();
export async function reconcile(candidate: Candidate): Promise<Candidate> {
  const publishing = candidate.status === 'publishing';
  if (!githubEnabled() || (!publishing && candidate.ciStatus !== 'pending')) return candidate;
  const now = Date.now();
  if (now - (reconciled.get(candidate.id) ?? 0) < 30000) return candidate;
  reconciled.set(candidate.id, now);
  // Bound the cache without persisting polling state as release evidence.
  if (reconciled.size > 1000) reconciled.delete(reconciled.keys().next().value!);
  const key = `${candidate.id}:${publishing ? 'publish' : 'verify'}`;
  const expected = store().maybe<{ workflowSha: string; at: string }>('dispatch', key);
  if (!expected) return candidate;
  try {
    const workflow = publishing ? 'verification-publish.yml' : 'verification-candidate.yml';
    const response = await github(`actions/workflows/${workflow}/runs?event=workflow_dispatch&branch=${encodeURIComponent(allowedBranch())}&per_page=100`);
    const run = response.workflow_runs.find((item: any) => item.display_title === `${publishing ? 'Publish' : 'Verification'} candidate ${candidate.id}` && item.head_sha === expected.workflowSha && Date.parse(item.created_at) >= Date.parse(expected.at) - 1000);
    if (!run) return candidate;
    if (store().get<{ at: string }>('dispatch', key).at !== expected.at) return store().candidate(candidate.id);
    return store().updateCandidate(candidate.id, (current) => {
      if (publishing && current.status === 'publishing') {
        if (run.status === 'completed') current.failure = `Publication workflow ${run.id} finished with ${run.conclusion}, but publication confirmation was not recorded. Inspect the workflow and retry interrupted publication when corrected.`;
      } else if (!publishing && current.ciStatus === 'pending') {
        if (run.status === 'completed') {
          current.ciStatus = 'failure';
          current.failure = `Verification workflow ${run.id} finished with ${run.conclusion}, but complete test evidence was not received. Inspect the workflow and prepare a new candidate after correcting it.`;
        } else if (run.status === 'in_progress') current.status = 'running';
      }
    });
  } catch (error) {
    // A GitHub outage must not change the evidence or invent a failed test.
    console.error('Could not refresh workflow state', error instanceof Error ? error.message : 'Unknown error');
    return store().candidate(candidate.id);
  }
}
