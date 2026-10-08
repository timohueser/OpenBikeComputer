<script context="module" lang="ts">
  const deviceKey = 'obc-verification-device';
  function lastDevice() { try { return sessionStorage.getItem(deviceKey) || ''; } catch { return ''; } }
  function rememberDevice(value: string) { try { sessionStorage.setItem(deviceKey, value); } catch { /* storage is optional */ } }
</script>
<script lang="ts">
  import type { Candidate, Requirement, VerificationTest, Attachment, ManualRun } from '$lib/types';
  import { api, date, message } from './api';
  import Markdown from './Markdown.svelte';
  import MarkdownField from './MarkdownField.svelte';
  import Files from './Files.svelte';
  import RequirementLabels from './RequirementLabels.svelte';
  export let candidate: Candidate;
  export let requirement: Requirement;
  export let test: VerificationTest;
  export let onsaved: (candidate: Candidate) => void;
  export let onback: () => void;
  export let dirty = false;
  let remembered = lastDevice();
  let device = remembered;
  let result: ManualRun['result'] | '' = '';
  let notes = '';
  let evidence: Attachment[] = [];
  let busy = false;
  let uploading = false;
  let error = '';
  $: runs = candidate.manualRuns.filter(r => r.testId === test.id && r.requirementId === requirement.id).slice().reverse();
  $: dirty = !!((device && device !== remembered) || result || notes || evidence.length);
  $: readonly = !!candidate.evidenceFrozen || candidate.status === 'published' || candidate.status === 'publishing';
  async function save() {
    busy = true; error = '';
    try {
      candidate = await api<Candidate>(`/api/candidates/${candidate.id}/runs`, 'POST', { requirementId: requirement.id, testId: test.id, device, result, notes, evidence });
      rememberDevice(remembered = device); result = ''; notes = ''; evidence = []; dirty = false; onsaved(candidate); onback();
    } catch (e) { error = message(e); } finally { busy = false; }
  }
</script>
<button class="back" on:click={() => { if (!dirty || confirm('Discard this unsaved test result?')) { dirty = false; onback(); } }}>← Back to candidate</button>
<div class="eyebrow">{requirement.id} · {requirement.group || 'Ungrouped'} · Manual verification</div><h2 class="requirement-title">{test.title}</h2><RequirementLabels {requirement} />{#if requirement.todo}<p class="warning small">This requirement has an incomplete definition. Its definition must be completed in a new candidate before publication.</p>{/if}<p class="muted small">Version {candidate.version} · <code>{candidate.sourceSha.slice(0,10)}</code> · requirements r{candidate.revision.id}</p>
<div class="section"><h3>Procedure</h3><Markdown text={test.steps || ''} /><h3>Expected result</h3><Markdown text={test.expected || ''} /><Files label="Input files for this candidate" files={test.inputs} /></div>
{#if runs[0]}<div class="inset" class:exception-record={runs[0].result !== 'pass'}><div class="row"><h3>Last run: <span class:success={runs[0].result === 'pass'} class:error={runs[0].result === 'fail'} class:warning={runs[0].result === 'blocked'}>{runs[0].result}</span></h3><span class="small muted">{date(runs[0].createdAt)} · {runs[0].author} · {runs[0].device}</span></div>{#if runs[0].notes}<Markdown text={runs[0].notes} />{/if}{#if runs[0].evidence.length}<p class="small muted">{runs[0].evidence.length} evidence {runs[0].evidence.length === 1 ? 'file' : 'files'} in the run history below.</p>{/if}</div>{/if}
{#if !readonly}<form class="section" on:submit|preventDefault={save}><h2>{runs[0] ? 'Record a new result' : 'Record result'}</h2><div class="columns"><label>Device / hardware revision<input required bind:value={device} placeholder="For example: prototype 2, firmware shown above" /></label><label>Result<select required bind:value={result}><option value="">Choose a result</option><option value="pass">Pass</option><option value="fail">Fail</option><option value="blocked">Blocked</option></select></label></div><MarkdownField label={result === 'pass' ? 'Notes (optional)' : 'Notes'} bind:value={notes} required={result === 'fail' || result === 'blocked'} rows={3} /><Files label="Evidence" bind:files={evidence} editable onbusy={(value) => uploading = value} /><p class="muted small">Record result saves this result immediately. You cannot change it. A later run replaces it as the current result, and the history keeps both.</p>{#if error}<p class="error" role="alert">{error}</p>{/if}<button class="primary" disabled={busy || uploading}>{busy ? 'Recording…' : 'Record result'}</button></form>{/if}
<div class="section"><h2>Run history <span class="muted">{runs.length}</span></h2>{#each runs as run}<details class="test"><summary><span class:success={run.result === 'pass'} class:error={run.result === 'fail'} class:warning={run.result === 'blocked'}>{run.result}</span> · {date(run.createdAt)} · {run.author}</summary><p class="small">Device: {run.device}</p><Markdown text={run.notes} /><Files label="Evidence" files={run.evidence} /></details>{:else}<p class="muted">No result has been recorded for this candidate.</p>{/each}</div>
