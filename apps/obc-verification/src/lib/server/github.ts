import { assert, positive, text, Problem } from './domain.ts';
import { store } from './store.ts';
import type { Candidate } from '../types.ts';

interface Dispatch { id: string; at: string; status: 'unknown' | 'rejected' | 'accepted'; runId?: number }
class GitHubFailure extends Problem {
  responseStatus: number;
  constructor(status: number) { super(502, `GitHub request failed (${status}).`); this.responseStatus = status; }
}
export const githubEnabled = () => !!(process.env.GITHUB_TOKEN && process.env.GITHUB_REPOSITORY);
export const allowedBranch = () => process.env.VERIFICATION_SOURCE_BRANCH || 'develop';
export async function github(path: string, body?: unknown): Promise<any> {
  assert(githubEnabled(), 'GitHub integration is not configured.', 503);
  const response = await fetch(`https://api.github.com/repos/${process.env.GITHUB_REPOSITORY}/${path}`, {
    method: body === undefined ? 'GET' : 'POST', headers: { Authorization: `Bearer ${process.env.GITHUB_TOKEN}`, Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
    body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(30000)
  });
  if (!response.ok) throw new GitHubFailure(response.status);
  return response.status === 204 ? undefined : response.json();
}
export async function sourceCommit(ref: string): Promise<{ sourceSha: string }> {
  const branch = allowedBranch();
  assert(ref === branch || /^[a-f0-9]{40}$/.test(ref), `Choose ${branch} or an exact commit from that branch.`);
  const head = await github(`commits/${encodeURIComponent(branch)}`);
  const source = ref === branch ? head : await github(`commits/${ref}`);
  if (source.sha !== head.sha) {
    const comparison = await github(`compare/${source.sha}...${head.sha}`);
    assert(comparison.status === 'ahead' || comparison.status === 'identical', `Source commit must be part of ${branch}.`);
  }
  return { sourceSha: source.sha };
}
export async function dispatch(candidate: Candidate, publish = false): Promise<void> {
  const workflow = publish ? 'verification-publish.yml' : 'verification-candidate.yml';
  const key = `${candidate.id}:${publish ? 'publish' : 'verify'}`;
  const record: Dispatch = { id: store().id(), at: new Date().toISOString(), status: 'unknown' };
  store().put('dispatch', key, record);
  try {
    if (!githubEnabled()) { record.status = 'rejected'; throw new Problem(503, 'GitHub integration is not configured.'); }
    const response = await github(`actions/workflows/${workflow}/dispatches`, { ref: allowedBranch(), return_run_details: true,
      inputs: publish ? { candidate_id: candidate.id } : { candidate_id: candidate.id, source_sha: candidate.sourceSha, release_version: candidate.version } });
    record.runId = positive(response?.workflow_run_id, 'Dispatched run ID');
    record.status = 'accepted';
    store().put('dispatch', key, record);
  } catch (error) {
    if (error instanceof GitHubFailure && error.responseStatus >= 400 && error.responseStatus < 500 && error.responseStatus !== 408) record.status = 'rejected';
    store().put('dispatch', key, record);
    throw error;
  }
}
function trustedRun(candidate: Candidate, run: Record<string, any>, expected: Dispatch, publish: boolean): void {
  const path = `.github/workflows/${publish ? 'verification-publish' : 'verification-candidate'}.yml`;
  assert(expected.status === 'accepted' && run.id === expected.runId && run.event === 'workflow_dispatch' && run.path === path && run.head_branch === allowedBranch() && run.display_title === `${publish ? 'Publish' : 'Verification'} candidate ${candidate.id}`, 'Workflow provenance does not match the candidate.', 403);
}
export async function verifyRun(candidate: Candidate, body: Record<string, any>, publish = false): Promise<void> {
  assert(body.sourceSha === candidate.sourceSha, 'Candidate source does not match.', 409);
  const runId = positive(body.runId, 'Run ID');
  const attempt = positive(body.runAttempt, 'Run attempt');
  const run = await github(`actions/runs/${runId}/attempts/${attempt}`);
  const expected = store().get<Dispatch>('dispatch', `${candidate.id}:${publish ? 'publish' : 'verify'}`);
  assert(runId === expected.runId, 'Workflow run ID does not match the dispatch.', 403);
  trustedRun(candidate, run, expected, publish);
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
  const previous = store().maybe<Dispatch>('dispatch', `${candidate.id}:publish`);
  if (previous?.status === 'rejected') return;
  assert(previous?.status === 'accepted' && previous.runId, 'Publication dispatch outcome is uncertain. Check GitHub before retrying.', 409);
  const run = await github(`actions/runs/${previous.runId}`);
  trustedRun(candidate, run, previous, true);
  assert(run.status === 'completed', 'Publication is still running.', 409);
  assert(['failure', 'cancelled', 'timed_out', 'action_required'].includes(run.conclusion), 'A publication workflow passed. Reconcile its result before retrying.', 409);
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
  const expected = store().maybe<Dispatch>('dispatch', key);
  if (expected?.status !== 'accepted' || !expected.runId) return candidate;
  try {
    const run = await github(`actions/runs/${expected.runId}`);
    trustedRun(candidate, run, expected, publishing);
    if (store().get<Dispatch>('dispatch', key).id !== expected.id) return store().candidate(candidate.id);
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
