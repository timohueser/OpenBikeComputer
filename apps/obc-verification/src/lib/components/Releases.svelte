<script lang="ts">
  import { onMount } from 'svelte';
  import type { Actor, Revision, Candidate, Readiness, Requirement, VerificationTest } from '$lib/types';
  import { api, ApiError, date, message } from './api';
  import Markdown from './Markdown.svelte';
  import Files from './Files.svelte';
  import ManualCheck from './ManualCheck.svelte';
  import RequirementException from './RequirementException.svelte';
  export let actor: Actor;
  export let revision: Revision;
  export let candidates: Candidate[];
  export let configured: boolean;
  export let requirementsDirty = false;
  export let dirty = false;
  let candidate: Candidate | null = null;
  let readiness: Readiness | null = null;
  let preparing = false;
  let version = '';
  let sourceRef = 'develop';
  let error = '';
  let busy = false;
  let review = false;
  let check: { requirement: Requirement; test: VerificationTest } | null = null;
  let checked = false;
  let lastRefresh = '';
  let request = 0;
  let manualDirty = false;
  let exceptionDrafts: Record<string, boolean> = {};
  let exceptionBusy = false;
  let editorEpoch = 0;
  $: dirty = manualDirty || Object.values(exceptionDrafts).some(Boolean);
  $: hasExceptions = !!candidate?.exceptions?.length;
  $: locked = !!candidate?.evidenceFrozen || candidate?.status === 'published' || candidate?.status === 'publishing';
  function back() { if (exceptionBusy) return false; if (dirty && !confirm('Discard the unsaved test result or exception reason?')) return false; manualDirty = false; exceptionDrafts = {}; editorEpoch++; check = null; review = false; preparing = false; return true; }
  function exceptionDirty(id: string, value: boolean) { if (!!exceptionDrafts[id] !== value) exceptionDrafts = { ...exceptionDrafts, [id]: value }; }
  function exceptionWorking(value: boolean) { exceptionBusy = value; if (value) { request++; busy = false; } }
  async function exceptionChanged(value: Candidate) { candidate = value; remember(value); await load(value.id, true); }
  function remember(c: Candidate) { candidates = [c, ...candidates.filter(value => value.id !== c.id)].sort((a,b) => b.createdAt.localeCompare(a.createdAt)); }
  async function load(id: string, background = false) {
    if (!background && !back()) return;
    const token = ++request;
    if (!background) { busy = true; error = ''; }
    try {
      const data = await api<{candidate: Candidate; readiness: Readiness}>(`/api/candidates/${id}`);
      if (token !== request) return;
      candidate = data.candidate; readiness = data.readiness; remember(candidate); lastRefresh = new Date().toLocaleTimeString();
    } catch (e) { if (token === request) error = message(e); }
    finally { if (token === request) busy = false; }
  }
  onMount(() => {
    if (candidates.length) load(candidates[0].id);
    const timer = setInterval(() => { if (candidate && !preparing && !review && !busy && !exceptionBusy && ['queued','running','publishing'].includes(candidate.status)) load(candidate.id, true); }, 15000);
    return () => clearInterval(timer);
  });
  async function prepare() {
    busy = true; error = '';
    try { candidate = await api<Candidate>('/api/candidates', 'POST', {version, sourceRef, revisionId: revision.id}); remember(candidate); preparing = false; version = ''; await load(candidate.id); }
    catch (e) { error = message(e); } finally { busy = false; }
  }
  async function publish() {
    if (!candidate) return;
    busy = true; error = '';
    try { candidate = await api<Candidate>(`/api/candidates/${candidate.id}/publish`, 'POST', { exceptions: candidate.exceptions ?? [] }); remember(candidate); review = false; checked = false; await load(candidate.id); }
    catch (e) {
      const failure = message(e);
      if (e instanceof ApiError && e.status === 409) { review = false; checked = false; await load(candidate.id, true); }
      error = failure;
    } finally { busy = false; }
  }
  function status(r: Requirement, t: VerificationTest) { if (!candidate) return 'pending'; return t.kind === 'automated' ? candidate.results.find(v => v.caseId === t.caseId)?.status || 'pending' : candidate.manualRuns.filter(v => v.requirementId === r.id && v.testId === t.id).at(-1)?.result || 'pending'; }
</script>
<div class="page-heading row"><div><div class="eyebrow">From candidate to release</div><h1>Releases</h1><p class="muted">One build. Its checks. A preserved record.</p></div><button class="primary" disabled={exceptionBusy} on:click={() => { if (back()) { preparing = true; error = ''; } }}>Prepare release</button></div>
{#if error}<div class="alert error" role="alert">{error}</div>{/if}
<div class="workbench"><aside><div class="eyebrow sidebar-caption">Release candidates</div>{#each candidates as c}<button class="entry" disabled={exceptionBusy} class:selected={!preparing && candidate?.id === c.id} on:click={() => load(c.id)}><strong>{c.version}</strong><span class="small muted">{c.status} · {c.sourceSha.slice(0,8)}</span><span class="small muted">{date(c.createdAt)}</span></button>{:else}<p class="muted small">Prepared candidates will appear here, including failed attempts.</p>{/each}</aside><section class="detail">
{#if preparing}
  <button class="back" on:click={() => preparing = false}>← Back to releases</button><h2 class="requirement-title">Prepare a release candidate</h2><p class="muted">Fix a code revision and a requirements revision, then build and run verification in GitHub Actions.</p>
  {#if !configured}<div class="alert warning">GitHub release integration is not configured. A maintainer must configure the repository connection before preparing a candidate.</div>{/if}
  {#if requirementsDirty}<div class="alert warning">You have unsaved requirement changes. Save or discard them in Requirements before preparing a candidate.</div>{/if}
  <form class="section" on:submit|preventDefault={prepare}><div class="columns"><label>Release version<input required bind:value={version} placeholder="0.1.0" /></label><label>Source branch or commit<input required bind:value={sourceRef} placeholder="develop" /></label></div><div class="inset"><strong>Requirements r{revision.id}</strong><p class="small muted">{revision.requirements.filter(r => r.active).length} active requirements · saved by {revision.author} · {date(revision.createdAt)}</p></div>{#if revision.requirements.some(r => r.active && r.todo)}<p class="warning small">Active To do requirements block publication. Complete their definitions and prepare a new candidate before release.</p>{/if}<p class="muted small">The server resolves and validates the source commit. Results belong only to this candidate. Preparing does not publish a release.</p><button class="primary" disabled={busy || !configured || requirementsDirty || !revision.requirements.some(r => r.active)}>{busy ? 'Preparing…' : 'Build & verify candidate'}</button>{#if !revision.requirements.some(r => r.active)}<p class="warning small">Activate and save at least one requirement first.</p>{/if}</form>
{:else if candidate && check}
  {#key candidate.id + check.test.id}<ManualCheck {candidate} requirement={check.requirement} test={check.test} bind:dirty={manualDirty} onback={() => check = null} onsaved={(c) => { candidate = c; remember(c); load(c.id, true); }} />{/key}
{:else if candidate && review}
  <button class="back" on:click={() => review = false}>← Back to candidate</button><div class="eyebrow">Final review</div><h2 class="requirement-title">Publish {candidate.version}</h2><p>The tested firmware and verification report will become the release record.</p><div class="section"><div class="review-row"><span>Code revision</span><code>{candidate.sourceSha}</code></div><div class="review-row"><span>Requirements</span><strong>r{candidate.revision.id}</strong></div><div class="review-row"><span>Verification</span><strong>{readiness?.verified} of {readiness?.total} verified · accepted exceptions: {readiness?.excepted || 0}</strong></div></div>{#if hasExceptions}<div class="inset exception-record"><h3 class="warning">Ready with exceptions</h3><p class="small">These requirements are accepted for this candidate; they are not verified.</p>{#each candidate.exceptions || [] as exception}<div class="test"><strong>{exception.requirementId} · {candidate.revision.requirements.find(r => r.id === exception.requirementId)?.title}</strong><p class="exception-reason">{exception.reason}</p><p class="small muted">Accepted by {exception.author} · {date(exception.createdAt)}</p></div>{/each}</div>{/if}<div class="section"><Files label="Tested release files" files={candidate.assets} /></div><p><a href={`/api/candidates/${candidate.id}/report`} target="_blank" rel="noreferrer">Review complete verification report ↗</a></p><label class="check"><input type="checkbox" bind:checked={checked} />{hasExceptions ? 'I have reviewed this candidate and its evidence, and accept the listed exceptions for this release.' : 'I have reviewed this candidate and its evidence.'}</label><button class="primary" disabled={busy || !checked || !readiness?.ready} on:click={publish}>{busy ? 'Starting publication…' : 'Publish release'}</button><p class="small muted">The server checks readiness again before publication. The tested files are promoted without a rebuild.</p>
{:else if candidate}
  <div class="row"><div class="eyebrow">Release candidate</div><span class="badge" class:success={(candidate.status === 'published' || readiness?.ready) && !hasExceptions} class:warning={hasExceptions && candidate.status !== 'failed'} class:error={candidate.status === 'failed'}>{candidate.status === 'published' && hasExceptions ? 'Published with exceptions' : readiness?.ready && hasExceptions ? 'Ready with exceptions' : candidate.status}</span></div><h2 class="requirement-title">Version {candidate.version}</h2><p class="muted small">{candidate.sourceRef} · <code title={candidate.sourceSha}>{candidate.sourceSha.slice(0,12)}</code> · requirements r{candidate.revision.id}<br>Prepared {date(candidate.createdAt)}</p>
  <div class="actions"><button disabled={busy || exceptionBusy} on:click={() => load(candidate!.id, true)}>{busy ? 'Refreshing…' : 'Refresh results'}</button><a class="button" href={`/api/candidates/${candidate.id}/report`} target="_blank" rel="noreferrer">View report</a><a class="button" href={`/api/candidates/${candidate.id}/report?download=1`} download={`verification-${candidate.version}.html`}>Download report</a>{#if candidate.releaseUrl}<a class="button" href={candidate.releaseUrl} target="_blank" rel="noreferrer">Open release ↗</a>{/if}</div>
  {#if lastRefresh}<p class="muted small" aria-live="polite">Updated {lastRefresh}{['queued','running','publishing'].includes(candidate.status) ? ' · refreshes every 15 seconds' : ''}</p>{/if}
  {#if candidate.failure}<div class="alert error">{candidate.failure}</div>{/if}
  {#if candidate.revision.id !== revision.id}<p class="muted small">Newer requirements exist. This candidate retains r{candidate.revision.id}.</p>{/if}
  <div class="inset" class:exception-record={hasExceptions}><div class="row"><h3 class:warning={hasExceptions}>{candidate.status === 'published' ? hasExceptions ? 'Published with exceptions' : 'Release published' : readiness?.ready ? hasExceptions ? 'Ready with exceptions' : 'Ready for publication' : 'Verification incomplete'}</h3><span class="small">{readiness?.verified || 0} / {readiness?.total || 0} verified · accepted exceptions: {readiness?.excepted || 0}</span></div><p class="small muted">Automated workflow: {candidate.ciStatus}</p>{#if hasExceptions}<p class="small warning">Accepted exceptions apply only to this candidate. They do not override failed CI, signing, or missing release files.</p>{/if}{#if readiness?.missing.length}<details><summary class="warning small">{readiness.missing.length} outstanding {readiness.missing.length === 1 ? 'item' : 'items'}</summary><ul class="small">{#each readiness.missing as item}<li>{item}</li>{/each}</ul></details>{/if}</div>
  <div class="section"><h2>Requirement evidence</h2>{#key candidate.id + ':' + editorEpoch}{#each candidate.revision.requirements.filter(r => r.active) as r (r.id)}<section class="requirement-check"><div class="eyebrow">{r.id} · {r.group || 'Ungrouped'}</div><h3>{r.title} {#if r.todo}<span class="todo-label">To do</span>{/if}</h3>{#if r.todo}<p class="warning small">Definition incomplete. Publication is blocked; this requirement cannot be excepted.</p>{/if}<details class="small"><summary>Read requirement</summary><Markdown text={r.statement} /></details>{#each r.tests as t}{@const outcome = status(r,t)}<div class="test"><div class="row"><div><span class="eyebrow">{t.kind}</span><h4>{t.title}</h4></div><div class="actions"><span class="badge" class:success={outcome === 'pass'} class:error={outcome === 'fail' || outcome === 'error'} class:warning={outcome !== 'pass' && outcome !== 'fail' && outcome !== 'error'}>{outcome}</span>{#if t.kind === 'manual'}<button disabled={exceptionBusy} on:click={() => { if (back()) check = {requirement:r,test:t}; }}>{locked ? 'View runs' : outcome === 'pending' ? 'Run check' : 'View / run again'}</button>{/if}</div></div>{#if t.kind === 'automated'}<code class="small wrap">{t.caseId}</code>{#if candidate.results.find(v => v.caseId === t.caseId)?.detail}<details class="small"><summary>Test output</summary><pre>{candidate.results.find(v => v.caseId === t.caseId)?.detail}</pre></details>{/if}{/if}</div>{:else}<p class="warning small">No verification defined.</p>{/each}<RequirementException {candidate} requirement={r} admin={!!actor.admin} disabled={locked || busy || exceptionBusy} onchanged={exceptionChanged} ondirty={exceptionDirty} onbusy={exceptionWorking} /></section>{:else}<p class="warning">This candidate has no active requirements.</p>{/each}{/key}</div>
  <div class="section"><Files label="Tested build" files={candidate.assets} /></div>
  {#if candidate.evidenceFrozen && candidate.status !== 'published'}<p class="muted small">Evidence is frozen because publication has started. A retry uses this exact evidence snapshot.</p>{/if}
  {#if candidate.status === 'publishing'}<div class="section"><button disabled={busy} on:click={publish}>Retry interrupted publication</button><p class="small muted">Retry only after the previous workflow has failed or stopped. The server checks that no publication is still running.</p></div>{/if}
  {#if candidate.status !== 'publishing' && candidate.status !== 'published'}<div class="section row"><span class="muted small">Publication requires verified requirements or accepted exceptions, plus passing CI and build checks.</span><button class="primary" disabled={!readiness?.ready || busy || exceptionBusy} on:click={() => { if (back()) { request++; review = true; checked = false; } }}>Review & publish</button></div><button class="text-button" disabled={exceptionBusy} on:click={() => { if (back()) { preparing = true; version = candidate!.version; } }}>Prepare another candidate</button>{/if}
{:else}<div class="empty"><h2>{busy ? 'Loading candidate…' : 'A release starts with a candidate.'}</h2><p class="muted">Prepare a fixed build, collect its automated and manual evidence, then publish once verification and any exceptions are reviewed.</p>{#if !busy}<button class="primary" on:click={() => preparing = true}>Prepare release</button>{/if}</div>{/if}
</section></div>
