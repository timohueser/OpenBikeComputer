<script lang="ts">
  import { onMount } from 'svelte';
  import type { Revision, Candidate, Readiness, Requirement, VerificationTest } from '$lib/types';
  import { api, date, message } from './api';
  import Markdown from './Markdown.svelte';
  import Files from './Files.svelte';
  import ManualCheck from './ManualCheck.svelte';
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
  $: locked = !!candidate?.evidenceFrozen || candidate?.status === 'published' || candidate?.status === 'publishing';
  function back() { if (dirty && !confirm('Discard this unsaved manual test result?')) return false; dirty = false; check = null; review = false; preparing = false; return true; }
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
    const timer = setInterval(() => { if (candidate && !preparing && !review && !busy && ['queued','running','publishing'].includes(candidate.status)) load(candidate.id, true); }, 15000);
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
    try { candidate = await api<Candidate>(`/api/candidates/${candidate.id}/publish`, 'POST'); remember(candidate); review = false; checked = false; await load(candidate.id); }
    catch (e) { error = message(e); } finally { busy = false; }
  }
  function status(r: Requirement, t: VerificationTest) { if (!candidate) return 'pending'; return t.kind === 'automated' ? candidate.results.find(v => v.caseId === t.caseId)?.status || 'pending' : candidate.manualRuns.filter(v => v.requirementId === r.id && v.testId === t.id).at(-1)?.result || 'pending'; }
</script>
<div class="page-heading row"><div><div class="eyebrow">From candidate to release</div><h1>Releases</h1><p class="muted">One build. Its checks. A preserved record.</p></div><button class="primary" on:click={() => { if (back()) { preparing = true; error = ''; } }}>Prepare release</button></div>
{#if error}<div class="alert error" role="alert">{error}</div>{/if}
<div class="workbench"><aside><div class="eyebrow sidebar-caption">Release candidates</div>{#each candidates as c}<button class="entry" class:selected={!preparing && candidate?.id === c.id} on:click={() => load(c.id)}><strong>{c.version}</strong><span class="small muted">{c.status} · {c.sourceSha.slice(0,8)}</span><span class="small muted">{date(c.createdAt)}</span></button>{:else}<p class="muted small">Prepared candidates will appear here, including failed attempts.</p>{/each}</aside><section class="detail">
{#if preparing}
  <button class="back" on:click={() => preparing = false}>← Back to releases</button><h2 class="requirement-title">Prepare a release candidate</h2><p class="muted">Fix a code revision and a requirements revision, then build and run verification in GitHub Actions.</p>
  {#if !configured}<div class="alert warning">GitHub release integration is not configured. A maintainer must configure the repository connection before preparing a candidate.</div>{/if}
  {#if requirementsDirty}<div class="alert warning">You have unsaved requirement changes. Save or discard them in Requirements before preparing a candidate.</div>{/if}
  <form class="section" on:submit|preventDefault={prepare}><div class="columns"><label>Release version<input required bind:value={version} placeholder="0.1.0" /></label><label>Source branch or commit<input required bind:value={sourceRef} placeholder="develop" /></label></div><div class="inset"><strong>Requirements r{revision.id}</strong><p class="small muted">{revision.requirements.filter(r => r.active).length} active requirements · saved by {revision.author} · {date(revision.createdAt)}</p></div><p class="muted small">The server resolves and validates the source commit. Results belong only to this candidate. Preparing does not publish a release.</p><button class="primary" disabled={busy || !configured || requirementsDirty || !revision.requirements.some(r => r.active)}>{busy ? 'Preparing…' : 'Build & verify candidate'}</button>{#if !revision.requirements.some(r => r.active)}<p class="warning small">Activate and save at least one requirement first.</p>{/if}</form>
{:else if candidate && check}
  {#key candidate.id + check.test.id}<ManualCheck {candidate} requirement={check.requirement} test={check.test} bind:dirty onback={() => check = null} onsaved={(c) => { candidate = c; remember(c); load(c.id, true); }} />{/key}
{:else if candidate && review}
  <button class="back" on:click={() => review = false}>← Back to candidate</button><div class="eyebrow">Final review</div><h2 class="requirement-title">Publish {candidate.version}</h2><p>The tested firmware and verification report will become the release record.</p><div class="section"><div class="review-row"><span>Code revision</span><code>{candidate.sourceSha}</code></div><div class="review-row"><span>Requirements</span><strong>r{candidate.revision.id}</strong></div><div class="review-row"><span>Verification</span><strong>{readiness?.verified} of {readiness?.total} requirements verified</strong></div></div><div class="section"><Files label="Tested release files" files={candidate.assets} /></div><p><a href={`/api/candidates/${candidate.id}/report`} target="_blank" rel="noreferrer">Review complete verification report ↗</a></p><label class="check"><input type="checkbox" bind:checked={checked} />I have reviewed this candidate and its evidence.</label><button class="primary" disabled={busy || !checked || !readiness?.ready} on:click={publish}>{busy ? 'Starting publication…' : 'Publish release'}</button><p class="small muted">The server checks readiness again before publication. The tested files are promoted without a rebuild.</p>
{:else if candidate}
  <div class="row"><div class="eyebrow">Release candidate</div><span class="badge" class:success={candidate.status === 'published' || readiness?.ready} class:error={candidate.status === 'failed'}>{candidate.status}</span></div><h2 class="requirement-title">Version {candidate.version}</h2><p class="muted small">{candidate.sourceRef} · <code title={candidate.sourceSha}>{candidate.sourceSha.slice(0,12)}</code> · requirements r{candidate.revision.id}<br>Prepared {date(candidate.createdAt)}</p>
  <div class="actions"><button disabled={busy} on:click={() => load(candidate!.id, true)}>{busy ? 'Refreshing…' : 'Refresh results'}</button><a class="button" href={`/api/candidates/${candidate.id}/report`} target="_blank" rel="noreferrer">View report</a><a class="button" href={`/api/candidates/${candidate.id}/report?download=1`} download={`verification-${candidate.version}.html`}>Download report</a>{#if candidate.releaseUrl}<a class="button" href={candidate.releaseUrl} target="_blank" rel="noreferrer">Open release ↗</a>{/if}</div>
  {#if lastRefresh}<p class="muted small" aria-live="polite">Updated {lastRefresh}{['queued','running','publishing'].includes(candidate.status) ? ' · refreshes every 15 seconds' : ''}</p>{/if}
  {#if candidate.failure}<div class="alert error">{candidate.failure}</div>{/if}
  {#if candidate.revision.id !== revision.id}<p class="muted small">Newer requirements exist. This candidate retains r{candidate.revision.id}.</p>{/if}
  <div class="inset"><div class="row"><h3>{candidate.status === 'published' ? 'Release published' : readiness?.ready ? 'Ready for publication' : 'Verification in progress'}</h3><span class="small">{readiness?.verified || 0} / {readiness?.total || 0} verified</span></div><p class="small muted">Automated workflow: {candidate.ciStatus}</p>{#if readiness?.missing.length}<details><summary class="warning small">{readiness.missing.length} outstanding {readiness.missing.length === 1 ? 'item' : 'items'}</summary><ul class="small">{#each readiness.missing as item}<li>{item}</li>{/each}</ul></details>{/if}</div>
  <div class="section"><h2>Requirement evidence</h2>{#each candidate.revision.requirements.filter(r => r.active) as r}<section class="requirement-check"><div class="eyebrow">{r.id} · {r.group || 'Ungrouped'}</div><h3>{r.title}</h3><details class="small"><summary>Read requirement</summary><Markdown text={r.statement} /></details>{#each r.tests as t}{@const outcome = status(r,t)}<div class="test"><div class="row"><div><span class="eyebrow">{t.kind}</span><h4>{t.title}</h4></div><div class="actions"><span class="badge" class:success={outcome === 'pass'} class:error={outcome === 'fail' || outcome === 'error'} class:warning={outcome !== 'pass' && outcome !== 'fail' && outcome !== 'error'}>{outcome}</span>{#if t.kind === 'manual'}<button on:click={() => check = {requirement:r,test:t}}>{locked ? 'View runs' : outcome === 'pending' ? 'Run check' : 'View / run again'}</button>{/if}</div></div>{#if t.kind === 'automated'}<code class="small wrap">{t.caseId}</code>{#if candidate.results.find(v => v.caseId === t.caseId)?.detail}<details class="small"><summary>Test output</summary><pre>{candidate.results.find(v => v.caseId === t.caseId)?.detail}</pre></details>{/if}{/if}</div>{:else}<p class="warning small">No verification defined. Prepare a new candidate after adding checks.</p>{/each}</section>{:else}<p class="warning">This candidate has no active requirements.</p>{/each}</div>
  <div class="section"><Files label="Tested build" files={candidate.assets} /></div>
  {#if candidate.evidenceFrozen && candidate.status !== 'published'}<p class="muted small">Evidence is frozen because publication has started. A retry uses this exact evidence snapshot.</p>{/if}
  {#if candidate.status === 'publishing'}<div class="section"><button disabled={busy} on:click={publish}>Retry interrupted publication</button><p class="small muted">Retry only after the previous workflow has failed or stopped. The server checks that no publication is still running.</p></div>{/if}
  {#if candidate.status !== 'publishing' && candidate.status !== 'published'}<div class="section row"><span class="muted small">Publication requires complete, passing evidence.</span><button class="primary" disabled={!readiness?.ready || busy} on:click={() => { review = true; checked = false; }}>Review & publish</button></div><button class="text-button" on:click={() => { preparing = true; version = candidate!.version; }}>Prepare another candidate</button>{/if}
{:else}<div class="empty"><h2>{busy ? 'Loading candidate…' : 'A release starts with a candidate.'}</h2><p class="muted">Prepare a fixed build, collect its automated and manual evidence, then publish once every requirement is verified.</p>{#if !busy}<button class="primary" on:click={() => preparing = true}>Prepare release</button>{/if}</div>{/if}
</section></div>
