import { isDeepStrictEqual } from 'node:util';
import type { RequestEvent } from '@sveltejs/kit';
import { json } from '@sveltejs/kit';
import type { Candidate, Catalog, CoverageProposal, ManualRun, Role } from '../types.ts';
import { attachments, assert, identifier, positive, Problem, readiness, requirements, testResults, text, within } from './domain.ts';
import { store } from './store.ts';
import { createSession, localLogin, logout, oauthEnabled, sameOrigin, requireAdmin, changePassword, agentTokens, createAgentToken, revokeAgentToken } from './auth.ts';
import { dispatch, githubEnabled, sourceCommit, verifyPublished, verifyRun, publicationRetry, verifyCatalog, reconcile } from './github.ts';
import { boundedBody, download, upload } from './files.ts';
import { approveGitHubUser, removeGitHubUser } from './accounts.ts';
import { report } from './report.ts';
import { coverageConflict, coveragePlan, coverageStale, commitSha, draftCoverage, proposalProcedures } from './coverage-plan.ts';

async function body(event: RequestEvent): Promise<Record<string, any>> {
  assert(event.request.headers.get('content-type')?.includes('application/json'), 'JSON body required.', 415);
  try {
    const value = JSON.parse((await boundedBody(event.request, 24 * 1024 * 1024)).toString('utf8'));
    assert(value && typeof value === 'object' && !Array.isArray(value), 'JSON object required.');
    return value;
  } catch (error) { if (error instanceof Problem) throw error; throw new Problem(400, 'Invalid JSON.'); }
}
export async function api(event: RequestEvent): Promise<Response> {
  try { return await route(event); }
  catch (error) {
    if (!(error instanceof Problem)) console.error('Verification API error', error);
    return json({ error: error instanceof Problem ? error.message : 'The request failed. Try again or contact the administrator.', ...(error instanceof Problem && error.at ? { at: error.at } : {}) }, { status: error instanceof Problem ? error.status : 500 });
  }
}
async function route(event: RequestEvent): Promise<Response> {
  const path = event.params.path ?? '';
  const parts = path.split('/');
  const method = event.request.method;
  const write = !['GET', 'HEAD'].includes(method);
  const actor = event.locals.actor;
  if (path === 'login') {
    if (method === 'GET') return json({ oauth: oauthEnabled() });
    assert(method === 'POST', 'Method not allowed.', 405); sameOrigin(event.request);
    const data = await body(event);
    const user = localLogin(data.username, data.password, event.getClientAddress()); createSession(event.cookies, user);
    return json({ actor: user });
  }
  assert(actor, 'Please sign in.', 401);
  const allow = (...roles: Role[]) => assert(roles.includes(actor.role), 'This credential cannot perform this action.', 403);
  if (write && actor.role === 'owner') sameOrigin(event.request);
  if (path === 'logout' && method === 'POST') { allow('owner'); logout(event.cookies); return json({ ok: true }); }
  if (parts[0] === 'admin' && parts[1] === 'agent-tokens') {
    requireAdmin(actor);
    if (parts.length === 2 && method === 'GET') return json(agentTokens(actor));
    if (parts.length === 2 && method === 'POST') {
      const data = await body(event);
      return json(createAgentToken(actor, data.name, data.expiresAt), { status: 201, headers: { 'Cache-Control': 'no-store' } });
    }
    if (parts.length === 3 && method === 'DELETE') {
      revokeAgentToken(actor, identifier(parts[2])); return json({ ok: true });
    }
    throw new Problem(405, 'Method not allowed.');
  }
  if (path === 'admin/history') {
    requireAdmin(actor);
    if (method === 'GET') return json(store().historySummary());
    assert(method === 'POST', 'Method not allowed.', 405);
    const data = await body(event);
    const admin = requireAdmin(actor);
    assert(typeof data.clearCurrent === 'boolean', 'Choose whether to keep the current requirements.');
    assert(data.confirmation === (data.clearCurrent ? 'START FRESH' : 'CLEAR HISTORY'), 'Type the confirmation phrase exactly.');
    return json(store().clearHistory(positive(data.baseRevision, 'Base revision'), admin.name, data.clearCurrent));
  }
  if (parts[0] === 'users') {
    requireAdmin(actor);
    if (parts.length === 1 && method === 'POST') {
      const data = await body(event);
      await approveGitHubUser(actor, data.login, data.admin ?? false);
    } else if (parts.length === 2 && method === 'DELETE') {
      removeGitHubUser(actor, parts[1]); return json({ ok: true });
    } else assert(parts.length === 1 && method === 'GET', 'Method not allowed.', 405);
    return json({ users: store().githubUsers(), oauth: oauthEnabled() });
  }
  if (path === 'account/password' && method === 'POST') {
    requireAdmin(actor);
    const data = await body(event);
    changePassword(actor, data.currentPassword, data.newPassword, event.cookies);
    return json({ ok: true });
  }
  if (parts[0] === 'ci') { allow('ci'); return ci(event, parts.slice(1)); }
  if (actor.role === 'ci') assert(parts[0] === 'files' || (parts[0] === 'candidates' && ['report', 'evidence'].includes(parts[2]) && method === 'GET'), 'CI access is limited to evidence ingestion and release artifacts.', 403);
  if (path === 'bootstrap' && method === 'GET') return json({ actor, revision: store().latestRevision(), catalog: store().catalog(), candidates: store().list<Candidate>('candidate'), configured: { github: githubEnabled(), oauth: oauthEnabled(), ...(process.env.VERIFICATION_DEMO === '1' ? { demo: true } : {}) } });
  if (path === 'requirements/next-id' && method === 'POST') {
    allow('owner');
    const raw = event.request.body && event.request.headers.get('content-type')?.includes('application/json') ? (await boundedBody(event.request, 1024)).toString('utf8').trim() : '';
    let count = 1;
    if (raw) { try { count = JSON.parse(raw).count ?? 1; } catch { throw new Problem(400, 'Invalid JSON.'); } }
    assert(Number.isSafeInteger(count) && count >= 1 && count <= 1000, 'Count must be between 1 and 1000.');
    const ids = store().reserveRequirementIds(count);
    return json({ id: ids[0], ids });
  }
  if (path === 'requirements' && method === 'PUT') {
    allow('owner'); const data = await body(event);
    const list = draftCoverage(requirements(data.requirements, (id) => store().file(id)), data.requirements, store().catalog(), () => store().id());
    return json(store().saveRevision(positive(data.baseRevision, 'Base revision'), actor.name, list));
  }
  if (path === 'revisions' && method === 'GET') return json(store().revisions());
  if (parts[0] === 'revisions' && parts.length === 2 && method === 'GET') return json(store().revision(positive(Number(parts[1]), 'Revision')));
  if (path === 'catalog' && method === 'GET') return json(store().catalog());
  if (path === 'files' && method === 'POST') { allow('owner', 'ci'); return json(await upload(event.request), { status: 201 }); }
  if (parts[0] === 'files' && parts.length === 2 && method === 'GET') return download(identifier(parts[1]));
  if (parts[0] === 'coverage-proposals') { allow('agent', 'owner'); return coverageProposals(event, parts); }
  if (path === 'candidates' && method === 'GET') return json(store().list<Candidate>('candidate'));
  if (path === 'candidates' && method === 'POST') {
    allow('owner'); const data = await body(event);
    const version = `v${text(data.version, 'Version', 80).replace(/^v/, '')}`;
    assert(/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.test(version), 'Use a version such as 0.1.0 or 0.1.0-rc.1.');
    assert(!store().list<Candidate>('candidate').some((c) => c.version === version && ['publishing', 'published'].includes(c.status)), 'This version is already being published or published.', 409);
    const sourceRef = text(data.sourceRef, 'Source reference', 200);
    const revisionId = positive(data.revisionId, 'Revision');
    store().revision(revisionId);
    const { sourceSha } = await sourceCommit(sourceRef);
    const revision = store().revision(revisionId);
    const candidate: Candidate = { id: store().id(), version, sourceRef, sourceSha, revision, createdAt: new Date().toISOString(), status: 'queued', ciStatus: 'pending', results: [], manualRuns: [], assets: [], exceptions: [] };
    store().put('candidate', candidate.id, candidate);
    try { await dispatch(candidate); }
    catch (error) { return json(store().updateCandidate(candidate.id, (c) => { c.ciStatus = 'failure'; c.failure = error instanceof Error ? error.message : 'Dispatch failed.'; }), { status: 201 }); }
    return json(candidate, { status: 201 });
  }
  if (parts[0] === 'candidates' && parts.length >= 2) {
    const id = identifier(parts[1]);
    const candidate = store().candidate(id);
    if (parts.length === 2 && method === 'GET') { const refreshed = await reconcile(candidate); return json({ candidate: refreshed, readiness: readiness(refreshed) }); }
    if (['evidence', 'report'].includes(parts[2]) && method === 'GET') {
      const publication = store().maybe<Candidate>('publication', id);
      const files = publication ? store().maybe<{ html: string; evidence: string }>('publication-files', id) : undefined;
      assert(!publication || files, 'Frozen publication files are missing. Restore them before continuing.', 409);
      if (parts[2] === 'evidence') return new Response(files?.evidence ?? JSON.stringify({ candidate, readiness: readiness(candidate) }), { headers: { 'Content-Type': 'application/json', 'Content-Disposition': `attachment; filename="${id}-evidence.json"` } });
      return new Response(files?.html ?? report(candidate), { headers: { 'Content-Type': 'text/html; charset=utf-8', 'Content-Disposition': `${event.url.searchParams.has('download') ? 'attachment' : 'inline'}; filename="${id}-report.html"`, 'Content-Security-Policy': "default-src 'none'; style-src 'unsafe-inline'; sandbox" } });
    }
    if (parts[2] === 'exceptions' && ((parts.length === 3 && method === 'POST') || (parts.length === 4 && method === 'DELETE'))) {
      requireAdmin(actor);
      const data = method === 'POST' ? await body(event) : undefined;
      const requirementId = identifier(data?.requirementId ?? parts[3], 'Requirement ID');
      const reason = data ? text(data.reason, 'Exception reason', 5000) : undefined;
      return json(store().updateCandidate(id, (current) => {
        const admin = requireAdmin(actor);
        assert(!current.evidenceFrozen && !['publishing', 'published'].includes(current.status) && !store().maybe('publication', id), 'Published evidence is frozen.', 409);
        const requirement = current.revision.requirements.find((r) => r.id === requirementId);
        assert(requirement?.active, 'Active requirement not found in this candidate.', 404);
        assert(!requirement.todo, 'Complete the requirement definition before accepting an exception.', 409);
        const exists = current.exceptions?.some((entry) => entry.requirementId === requirementId);
        if (method === 'POST') {
          assert(!exists, 'An exception already exists. Remove it before recording a replacement.', 409);
          current.exceptions = [...(current.exceptions ?? []), { requirementId, reason: reason!, author: admin.name, createdAt: new Date().toISOString() }];
        } else {
          assert(exists, 'Exception not found.', 404);
          current.exceptions = current.exceptions!.filter((entry) => entry.requirementId !== requirementId);
        }
      }));
    }
    if (parts[2] === 'runs' && method === 'POST') {
      allow('owner'); const data = await body(event);
      const req = candidate.revision.requirements.find((r) => r.id === data.requirementId);
      assert(req && req.tests.some((t) => t.id === data.testId && t.kind === 'manual'), 'Manual test not found in this candidate.', 404);
      assert(['pass', 'fail', 'blocked'].includes(data.result), 'Invalid manual result.');
      const notes = typeof data.notes === 'string' ? data.notes.trim() : '';
      assert(notes.length <= 50000 && (data.result === 'pass' || notes), 'Add notes for failed or blocked checks.');
      const run: ManualRun = { id: store().id(), requirementId: req.id, testId: data.testId, result: data.result, device: text(data.device, 'Device', 1000), notes, evidence: attachments(data.evidence ?? [], (fileId) => store().file(fileId)), author: actor.name, createdAt: new Date().toISOString() };
      return json(store().updateCandidate(id, (c) => { assert(!store().maybe('publication', id), 'Published evidence is frozen.', 409); c.manualRuns.push(run); }));
    }
    if (parts[2] === 'publish' && method === 'POST') {
      allow('owner');
      const data = await body(event);
      assert(Array.isArray(data.exceptions), 'The reviewed exception list is required.');
      const exceptionSnapshot = (entries: { requirementId?: unknown; reason?: unknown; author?: unknown; createdAt?: unknown }[]) => JSON.stringify(entries.map((entry) => [entry?.requirementId, entry?.reason, entry?.author, entry?.createdAt]));
      const previousDispatch = store().maybe<{ id: string }>('dispatch', `${id}:publish`)?.id;
      if (candidate.status === 'publishing') await publicationRetry(candidate);
      const locked = store().updateCandidate(id, (c) => {
        assert(c.status !== 'published', 'Release is already published.', 409);
        assert(c.status === candidate.status && store().maybe<{ id: string }>('dispatch', `${id}:publish`)?.id === previousDispatch, 'Candidate changed. Reload before publishing.', 409);
        assert(exceptionSnapshot(c.exceptions ?? []) === exceptionSnapshot(data.exceptions), 'Exceptions changed. Reload the candidate and review again.', 409);
        assert(readiness(c).ready, 'Verification is incomplete.', 409);
        assert(!store().list<Candidate>('candidate').some((other) => other.id !== id && other.version === c.version && ['publishing', 'published'].includes(other.status)), 'Another candidate already owns this version.', 409);
        c.status = 'publishing'; c.evidenceFrozen = true; delete c.failure;
        if (!store().maybe('publication', id)) {
          store().put('publication', id, c);
          store().put('publication-files', id, { html: report(c), evidence: JSON.stringify({ candidate: c, readiness: readiness(c) }) });
        }
      });
      try { await dispatch(locked, true); }
      catch (error) { return json(store().updateCandidate(id, (c) => { c.failure = error instanceof Error ? error.message : 'Publish dispatch failed.'; })); }
      return json(locked);
    }
  }
  throw new Problem(404, 'Endpoint not found.');
}
async function coverageProposals(event: RequestEvent, parts: string[]): Promise<Response> {
  const actor = event.locals.actor!;
  if (parts.length === 1 && event.request.method === 'GET') {
    const revision = store().latestRevision();
    const revisions = new Map(store().revisions().map(r => [r.id, r]));
    const catalog = store().catalog();
    return json(store().list<CoverageProposal>('coverage-proposal').map(p => ({ ...p,
      requirement: revision.requirements.find(r => r.id === p.requirementId),
      ...(p.status === 'pending' ? { conflict: coverageConflict(p, revision, catalog), stale: coverageStale(p, revisions.get(p.baseRevision), revision) } : {})
    })));
  }
  assert(event.request.method === 'POST', 'Method not allowed.', 405);
  const data = await body(event);
  if (parts.length === 1) {
    const baseRevision = positive(data.baseRevision, 'Base revision');
    const revision = store().latestRevision();
    assert(revision.id === baseRevision, 'Requirements changed. Reload before proposing coverage.', 409);
    const requirementId = identifier(data.requirementId);
    const requirement = revision.requirements.find(r => r.id === requirementId);
    assert(requirement, 'Requirement not found.', 404);
    const sourceSha = commitSha(data.sourceSha);
    const { procedures, plan } = within(`${requirement.id} · ${requirement.title}`, { requirementId: requirement.id }, () => {
      const procedures = proposalProcedures(data.procedures, requirement);
      return { procedures, plan: coveragePlan(data.plan, requirement, store().catalog(), procedures) };
    });
    assert(procedures.every(p => plan.criteria.some(c => c.evidence.some(e => e.testId === p.id))), 'Every proposed procedure must be cited as evidence.');
    /** One pending proposal per requirement: an identical plan is reused, any other replaces every pending one. */
    const pending = store().list<CoverageProposal>('coverage-proposal').filter(p => p.status === 'pending' && p.requirementId === requirementId);
    const identical = pending.find(p => p.baseRevision === baseRevision && p.sourceSha === sourceSha && isDeepStrictEqual([p.plan, p.procedures ?? []], [plan, procedures]));
    if (identical) return json(identical);
    const proposal: CoverageProposal = { id: store().id(), baseRevision, requirementId, sourceSha, plan, ...(procedures.length ? { procedures } : {}),
      author: actor.name, ...(actor.agentToken ? { agentToken: actor.agentToken } : {}), createdAt: new Date().toISOString(), status: 'pending', ...(pending.length ? { supersedes: pending[0].id } : {}) };
    store().atomic(() => {
      for (const previous of pending) store().put('coverage-proposal', previous.id, { ...previous, status: 'superseded' });
      store().put('coverage-proposal', proposal.id, proposal);
    });
    return json(proposal, { status: 201 });
  }
  assert(parts.length === 2 && actor.role === 'owner', 'Only an owner may review coverage.', 403);
  assert(typeof data.accept === 'boolean', 'Accept must be boolean.');
  const feedback = data.feedback === undefined || data.feedback === '' ? undefined : text(data.feedback, 'Review feedback', 5000);
  const decided = store().decideCoverageProposal(identifier(parts[1]), actor.name, data.accept, feedback);
  return json({ ...decided, revision: store().latestRevision() });
}
async function ci(event: RequestEvent, parts: string[]): Promise<Response> {
  if (parts[0] === 'catalog' && event.request.method === 'POST') {
    const data = await body(event);
    await verifyCatalog(data);
    assert(/^[a-f0-9]{40}$/.test(data.sourceSha), 'Invalid source SHA.');
    assert(Array.isArray(data.cases) && data.cases.length <= 100000, 'Invalid catalogue.');
    const ids = new Set<string>();
    const cases: Catalog['cases'] = data.cases.map((entry: any) => {
      const id = text(entry.id, 'Case ID', 1000); assert(!ids.has(id), 'Duplicate catalogue identity.'); ids.add(id);
      return { id, suite: text(entry.suite, 'Suite', 200), name: text(entry.name, 'Case name', 1000), ...(entry.file ? { file: text(entry.file, 'File', 2000) } : {}) };
    });
    assert(Array.isArray(data.namespaces) && data.namespaces.length <= 1000, 'Catalogue namespaces are required.');
    const namespaces = new Set<string>(data.namespaces.map((value: unknown) => text(value, 'Namespace', 200)));
    assert(cases.every((entry) => namespaces.has(entry.suite)), 'Every case must belong to a reported namespace.');
    if (!namespaces.size) return json(store().catalog());
    const retained = store().catalog().cases.filter((entry) => !namespaces.has(entry.suite));
    assert(retained.every((entry) => !ids.has(entry.id)), 'Case identity conflicts across namespaces.');
    const catalog: Catalog = { sourceSha: data.sourceSha, updatedAt: new Date().toISOString(), cases: [...retained, ...cases] };
    store().put('catalog', 'current', catalog); return json(catalog);
  }
  assert(parts[0] === 'candidates' && parts[1], 'Endpoint not found.', 404);
  const id = identifier(parts[1]); const candidate = store().candidate(id);
  if (parts.length === 2 && event.request.method === 'GET') return json({ candidate, readiness: readiness(candidate) });
  assert(event.request.method === 'POST', 'Method not allowed.', 405);
  const data = await body(event);
  if (parts[2] === 'results') {
    assert(data.conclusion === 'success' || data.conclusion === 'failure', 'Invalid conclusion.');
    await verifyRun(candidate, data);
    const results = testResults(data.results);
    const assets = attachments(data.assets, (fileId) => store().file(fileId));
    assert(new Set(assets.map((a) => a.name)).size === assets.length, 'Duplicate asset names.');
    return json(store().updateCandidate(id, (c) => {
      assert(!['publishing', 'published'].includes(c.status), 'Candidate is frozen.', 409);
      assert(!c.runId, 'Results have already been recorded. Prepare a new candidate.', 409);
      c.runId = data.runId; c.runAttempt = data.runAttempt; c.results = results; c.assets = assets; c.ciStatus = data.conclusion;
      if (data.failure) c.failure = text(data.failure, 'Failure', 10000);
    }));
  }
  if (parts[2] === 'published') {
    assert(candidate.status === 'publishing' || candidate.status === 'published', 'Candidate is not publishing.', 409);
    assert(readiness(candidate).ready, 'Evidence is incomplete.', 409);
    const releaseUrl = await verifyPublished(candidate, data);
    return json(store().updateCandidate(id, (c) => { assert(readiness(c).ready, 'Evidence changed.', 409); c.status = 'published'; c.releaseUrl = releaseUrl; }));
  }
  throw new Problem(404, 'Endpoint not found.');
}
