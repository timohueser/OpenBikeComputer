<script lang="ts">
  import type { Catalog, CoverageEvidence, CoveragePlan, Requirement } from '$lib/types';
  import { clone } from './api';
  import { evidenceTest, mappedBy } from '$lib/coverage';
  import CoverageChanges from './CoverageChanges.svelte';
  export let requirement: Requirement;
  export let catalog: Catalog;
  export let busy: boolean;
  export let onsave: (plan: CoveragePlan) => void;
  export let oncancel: () => void;
  let draft: CoveragePlan = requirement.coverage ? { ...clone(requirement.coverage), removeTestIds: [] } : {
    sourceSha: catalog.sourceSha, conclusion: 'partial', rationale: '', criteria: [{ id: crypto.randomUUID(), statement: '', evidence: [], gap: '' }]
  };
  let preview = false;
  let picker: string | null = null;
  let search = '';
  let error = '';
  $: eligibleComplete = draft.criteria.length > 0 && draft.criteria.every(c => c.evidence.length > 0 && !c.gap.trim());
  $: available = catalog.cases.filter(c => `${c.name} ${c.suite} ${c.file ?? ''}`.toLowerCase().includes(search.toLowerCase()));
  $: unmapped = requirement.tests.filter(t => !mappedBy(draft, t));
  function addEvidence(id: string, evidence: CoverageEvidence) {
    const criterion = draft.criteria.find(c => c.id === id)!;
    criterion.evidence = [...criterion.evidence, evidence];
    const test = evidenceTest(requirement, evidence);
    draft = { ...draft, removeTestIds: draft.removeTestIds?.filter(id => id !== test?.id) };
    picker = null;
  }
  function toggleRemoval(id: string, remove: boolean) {
    draft = { ...draft, removeTestIds: remove ? [...(draft.removeTestIds ?? []), id] : draft.removeTestIds?.filter(t => t !== id) };
  }
  function review() {
    error = '';
    if (!/^[a-f0-9]{40}$/.test(draft.sourceSha)) error = 'Enter the full 40-character commit that you assessed.';
    else if (!draft.rationale.trim()) error = 'Explain your coverage assessment.';
    else if (!draft.criteria.length || draft.criteria.some(c => !c.statement.trim() || c.evidence.some(e => !e.rationale.trim()))) error = 'Each criterion needs a statement and each mapped test needs an explanation.';
    else if (draft.criteria.some(c => !c.evidence.length && !c.gap.trim())) error = 'Describe the remaining work for criteria without evidence.';
    else if (draft.conclusion === 'complete' && !eligibleComplete) error = 'Complete coverage needs evidence for every criterion and no remaining gaps.';
    else preview = true;
  }
</script>
<div class="editor" aria-label="Coverage editor">
  {#if error}<p class="alert error" role="alert">{error}</p>{/if}
  {#if preview}
    <h3>Review your coverage changes</h3>
    <CoverageChanges {requirement} plan={draft} {catalog} />
    <p class="small">Saving records your review and applies the displayed test-link changes. It does not record any test results.</p>
    <div class="actions"><button disabled={busy} on:click={() => preview = false}>Back to editing</button><button class="primary" disabled={busy} on:click={() => onsave(draft)}>Save and approve coverage</button><button disabled={busy} on:click={oncancel}>Cancel</button></div>
  {:else}
    <p class="muted small">Describe each part of the requirement, map the tests that support it, and record what remains. A test can support several criteria.</p>
    {#each draft.criteria as criterion, index (criterion.id)}
      <fieldset class="inset" disabled={busy}>
        <legend>Criterion {index + 1}</legend>
        <label>Acceptance criterion<textarea rows={2} maxlength={5000} bind:value={criterion.statement}></textarea></label>
        {#each criterion.evidence as evidence, i}
          <div class="evidence"><strong class="small wrap">{evidenceTest(requirement, evidence)?.title ?? catalog.cases.find(c => c.id === evidence.caseId)?.name ?? evidence.caseId ?? evidence.testId}</strong>
            <label>What this test proves<textarea rows={2} maxlength={5000} bind:value={evidence.rationale}></textarea></label>
            <button class="text-button" on:click={() => { criterion.evidence = criterion.evidence.filter((_, n) => n !== i); draft = draft; }}>Remove evidence</button>
          </div>
        {/each}
        <button on:click={() => { picker = criterion.id; search = ''; }}>Map a test</button>
        {#if picker === criterion.id}
          <div class="picker"><div class="row"><strong>Choose evidence</strong><button on:click={() => picker = null}>Close picker</button></div>
            <label>Search tests<input type="search" bind:value={search} placeholder="Test name, suite, or file" /></label>
            {#each requirement.tests.filter(t => t.kind === 'manual' && t.title.toLowerCase().includes(search.toLowerCase())) as t}
              <button class="choice" disabled={criterion.evidence.some(e => e.testId === t.id)} on:click={() => addEvidence(criterion.id, { testId: t.id, rationale: '' })}>Manual · {t.title}</button>
            {/each}
            {#each available.slice(0, 30) as c}<button class="choice" disabled={criterion.evidence.some(e => e.caseId === c.id)} on:click={() => addEvidence(criterion.id, { caseId: c.id, rationale: '' })}>{c.name}<span class="small muted">{c.suite}</span></button>{/each}
            {#if available.length > 30}<p class="small muted">Showing 30 tests. Refine the search to find another test.</p>{/if}
            <p class="small muted">Create manual procedures in “Linked tests and manual procedures” before editing coverage.</p>
          </div>
        {/if}
        <label>Remaining gap<textarea rows={2} maxlength={5000} bind:value={criterion.gap} placeholder="What test or behavior is still needed? Leave empty only when this criterion is covered."></textarea></label>
        <button class="text-button danger" on:click={() => { draft = { ...draft, criteria: draft.criteria.filter(c => c.id !== criterion.id) }; }}>Remove criterion</button>
      </fieldset>
    {/each}
    <button disabled={busy || draft.criteria.length >= 100} on:click={() => draft = { ...draft, criteria: [...draft.criteria, { id: crypto.randomUUID(), statement: '', evidence: [], gap: '' }] }}>Add criterion</button>
    {#if unmapped.length}<div class="inset"><h4>Tests without a criterion mapping</h4><p class="small muted">Keep these as additional checks, or explicitly unlink them. Removing evidence alone does not delete a test.</p>{#each unmapped as t}<label class="check wrap"><input type="checkbox" checked={draft.removeTestIds?.includes(t.id) ?? false} disabled={busy} on:change={e => toggleRemoval(t.id, e.currentTarget.checked)} />Unlink {t.title}{t.kind === 'manual' ? ' (remove manual procedure)' : ''}</label>{/each}</div>{/if}
    <label>Coverage assessment<select aria-label="Coverage assessment" bind:value={draft.conclusion} disabled={busy}><option value="partial">Partial — work remains</option><option value="complete" disabled={!eligibleComplete}>Complete — all obligations have evidence</option></select></label>
    <label>Assessment explanation<textarea rows={3} maxlength={10000} bind:value={draft.rationale}></textarea></label>
    <label>Assessed source commit<input class="commit" maxlength={40} bind:value={draft.sourceSha} /></label>
    <p class="small muted">Review applies to this exact commit. A release at another commit needs a fresh assessment. The latest catalogue is from {catalog.sourceSha.slice(0, 10) || 'an unknown commit'}.</p>
    <div class="actions"><button class="primary" disabled={busy} on:click={review}>Review changes</button><button disabled={busy} on:click={oncancel}>Cancel</button></div>
  {/if}
</div>
<style>
  .editor { margin-top: 18px; } fieldset { min-width: 0; margin-bottom: 18px; } legend { font-weight: 600; }
  .evidence { padding: 12px 0 12px 14px; border-left: 3px solid var(--line); margin: 12px 0; }
  .picker { padding: 12px; margin-top: 12px; background: var(--soft); border-radius: 7px; }
  .choice { display: block; width: 100%; text-align: left; margin-top: 7px; overflow-wrap: anywhere; white-space: normal; }
  .choice span { display: block; } .commit { font-family: monospace; }
</style>
