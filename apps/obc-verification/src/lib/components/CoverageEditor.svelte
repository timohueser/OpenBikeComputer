<script lang="ts">
  import { TEST_LEVELS, type AcceptanceCriterion, type Catalog, type CatalogCase, type CoverageEvidence, type Requirement, type TestLevel, type VerificationTest } from '$lib/types';
  import { coverageChanges, criterionCovered, evidenceTest } from '$lib/coverage';
  import { clone } from './api';
  import TestEditor from './TestEditor.svelte';
  /** Edits the requirement's draft plan and tests in place; the parent saves them with the revision. */
  export let requirement: Requirement;
  export let catalog: Catalog;
  export let busy: boolean;
  export let onchange: () => void;
  export let ondone: () => void;
  export let onrefresh: () => void;
  /** True while a manual procedure is open for editing. The parent blocks saving until it is kept or cancelled. */
  export let editing = false;
  let picker: string | null = null;
  let search = '';
  let manualDraft: VerificationTest | null = null;
  let procedureDraft: { criterionId: string; test: VerificationTest } | null = null;
  $: editing = !!manualDraft || !!procedureDraft;
  $: plan = requirement.coverage!;
  $: covered = plan.criteria.filter(c => criterionCovered(requirement, c)).length;
  $: manual = requirement.tests.filter(t => t.kind === 'manual' && t.title.toLowerCase().includes(search.toLowerCase()));
  $: automated = catalog.cases.filter(c => `${c.name} ${c.suite} ${c.file ?? ''}`.toLowerCase().includes(search.toLowerCase()));
  const name = (e: CoverageEvidence) => evidenceTest(requirement, e)?.title ?? e.caseId ?? e.testId;
  const focus = (node: HTMLElement) => node.focus();
  function changed() { requirement = requirement; onchange(); }
  /** The level picked for each criterion's next test, so a level chosen before the sentence is typed survives. */
  let levels: Record<string, TestLevel> = {};
  const level = (criterion: AcceptanceCriterion) => levels[criterion.id] ?? criterion.next?.level ?? 'system';
  /** The next test to build is kept only while it has a summary. */
  function setNext(criterion: AcceptanceCriterion, summary: string) {
    if (summary.trim()) criterion.next = { level: level(criterion), summary }; else delete criterion.next;
    changed();
  }
  function addEvidence(criterion: AcceptanceCriterion, evidence: CoverageEvidence) {
    criterion.evidence.push(evidence); picker = null; manualDraft = null; changed();
  }
  function addCase(criterion: AcceptanceCriterion, c: CatalogCase) {
    if (!requirement.tests.some(t => t.caseId === c.id)) requirement.tests.push({ id: crypto.randomUUID(), kind: 'automated', title: c.name, caseId: c.id, inputs: [] });
    addEvidence(criterion, { caseId: c.id, rationale: '' });
  }
  function keepManual(criterion: AcceptanceCriterion, test: VerificationTest) {
    requirement.tests.push(test);
    addEvidence(criterion, { testId: test.id, rationale: '' });
  }
  function keepProcedure(test: VerificationTest) {
    requirement.tests = requirement.tests.map(t => t.id === test.id ? test : t);
    procedureDraft = null; changed();
  }
  function cancelProcedure() {
    const stored = requirement.tests.find(t => t.id === procedureDraft?.test.id);
    if (JSON.stringify(stored) !== JSON.stringify(procedureDraft?.test) && !confirm('Discard these changes to the manual procedure?')) return;
    procedureDraft = null;
  }
  /** A removal that takes a manual procedure's last citation deletes the procedure. Ask first. */
  function confirmRemoval(lead: string, criteria: AcceptanceCriterion[]): boolean {
    const gone = coverageChanges(requirement, { ...plan, criteria }).deletedProcedures;
    if (!gone.length) return true;
    const files = (t: VerificationTest) => `${t.inputs.length} input ${t.inputs.length === 1 ? 'file' : 'files'}`;
    return confirm(`${lead} and delete the manual ${gone.length === 1 ? 'procedure' : 'procedures'} ${gone.map(t => `“${t.title}” with its steps and ${files(t)}`).join(', ')}?`);
  }
  function removeEvidence(criterion: AcceptanceCriterion, index: number) {
    const evidence = criterion.evidence.filter((_, n) => n !== index);
    if (!confirmRemoval('Remove this evidence', plan.criteria.map(c => c.id === criterion.id ? { ...c, evidence } : c))) return;
    if (procedureDraft?.criterionId === criterion.id && procedureDraft.test.id === criterion.evidence[index].testId) procedureDraft = null;
    criterion.evidence = evidence; changed();
  }
  function removeCriterion(criterion: AcceptanceCriterion) {
    const criteria = plan.criteria.filter(c => c.id !== criterion.id);
    if (!confirmRemoval('Remove this criterion', criteria)) return;
    if (procedureDraft?.criterionId === criterion.id) procedureDraft = null;
    plan.criteria = criteria; changed();
  }
</script>
<div class="editor" aria-label="Coverage editor">
  <p class="progress small"><strong>{covered} of {plan.criteria.length}</strong> {plan.criteria.length === 1 ? 'criterion' : 'criteria'} covered <span class="muted">· a criterion is covered once it has evidence and no gap</span></p>
  <div class="criteria">
    {#each plan.criteria as criterion, index (criterion.id)}
      {@const done = criterionCovered(requirement, criterion)}
      <fieldset class="criterion" class:done disabled={busy}>
        <span class="mark" aria-hidden="true">{done ? '✓' : ''}</span>
        <div class="body">
          <div class="head">
            <textarea class="statement" rows={1} maxlength={5000} aria-label={`Criterion ${index + 1}`} placeholder="What must be true? One checkable statement." bind:value={criterion.statement} on:input={changed}></textarea>
            <button class="text-button remove" title="Remove criterion" aria-label="Remove criterion" on:click={() => removeCriterion(criterion)}>×</button>
          </div>
          {#each criterion.evidence as evidence, i}
            {@const test = evidenceTest(requirement, evidence)}
            <div class="evidence">
              <div class="row"><strong class="small wrap">{name(evidence)}</strong><span class="actions">{#if test?.kind === 'manual'}<button class="text-button small" on:click={() => procedureDraft = { criterionId: criterion.id, test: clone(test) }}>Edit procedure</button>{/if}<button class="text-button small" on:click={() => removeEvidence(criterion, i)}>Remove</button></span></div>
              <div class="evidence-line">
                <select class="level-pick" aria-label="Test level" value={evidence.level ?? ''} on:change={e => { const v = e.currentTarget.value; if (v) evidence.level = v as TestLevel; else delete evidence.level; changed(); }}>
                  <option value="">level?</option>
                  {#each TEST_LEVELS as l}<option value={l}>{l}</option>{/each}
                </select>
                <input maxlength={5000} aria-label="What this test proves" placeholder="What does this test prove?" bind:value={evidence.rationale} on:input={changed} />
              </div>
              {#if procedureDraft && procedureDraft.criterionId === criterion.id && procedureDraft.test.id === test?.id}<TestEditor bind:test={procedureDraft.test} onsave={keepProcedure} oncancel={cancelProcedure} />{/if}
            </div>
          {/each}
          {#if picker === criterion.id}
            <div class="picker">
              {#if manualDraft}
                <TestEditor bind:test={manualDraft} onsave={test => keepManual(criterion, test)} oncancel={() => manualDraft = null} />
              {:else}
                <input type="search" use:focus aria-label="Search tests" bind:value={search} placeholder="Search tests by name, suite, or file…" on:keydown={e => { if (e.key === 'Escape') picker = null; }} />
                <div class="choices">
                  {#each manual as t}<button class="choice" disabled={criterion.evidence.some(e => e.testId === t.id)} on:click={() => addEvidence(criterion, { testId: t.id, rationale: '' })}>{t.title}<span class="small muted">Manual procedure</span></button>{/each}
                  {#each automated.slice(0, 30) as c}<button class="choice" disabled={criterion.evidence.some(e => e.caseId === c.id)} on:click={() => addCase(criterion, c)}>{c.name}<span class="small muted">{c.suite}</span></button>{/each}
                  {#if !manual.length && !automated.length}<p class="small muted">{catalog.cases.length ? 'No test matches.' : 'No CI catalogue yet. Automated tests appear after the first verification run.'}</p>{/if}
                  {#if automated.length > 30}<p class="small muted">Showing 30 tests. Refine the search to find another.</p>{/if}
                </div>
                <div class="row"><button class="text-button small" on:click={() => manualDraft = { id: crypto.randomUUID(), kind: 'manual', title: '', steps: '', expected: '', inputs: [] }}>+ New manual procedure</button><span class="actions"><button class="text-button small" disabled={busy} on:click={onrefresh}>Refresh catalogue</button><button class="text-button small" on:click={() => picker = null}>Close</button></span></div>
              {/if}
            </div>
          {:else}
            <button class="text-button add" on:click={() => { picker = criterion.id; search = ''; }}>+ Add evidence</button>
          {/if}
          <textarea class="gap" rows={1} maxlength={5000} aria-label="Remaining gap" placeholder={criterion.evidence.length ? 'Gap — leave empty if the evidence above is enough' : 'Gap — what is still missing? Optional.'} bind:value={criterion.gap} on:input={changed}></textarea>
          <div class="next"><select aria-label="Proposed test level" value={level(criterion)} on:change={e => { levels[criterion.id] = e.currentTarget.value as TestLevel; setNext(criterion, criterion.next?.summary ?? ''); }}>{#each TEST_LEVELS as level}<option value={level}>{level}</option>{/each}</select><input maxlength={300} aria-label="Proposed test" placeholder="Next test to build — one sentence, optional" value={criterion.next?.summary ?? ''} on:input={e => setNext(criterion, e.currentTarget.value)} /></div>
        </div>
      </fieldset>
    {/each}
  </div>
  <button disabled={busy || plan.criteria.length >= 100} on:click={() => { plan.criteria.push({ id: crypto.randomUUID(), statement: '', evidence: [], gap: '' }); changed(); }}>+ Add criterion</button>
  <p class="small muted note">The requirement's tests are exactly the tests this plan cites. A manual procedure that no criterion cites is deleted with its steps when you save the revision.</p>
  <label>Summary <span class="muted">· optional</span><textarea rows={2} maxlength={10000} bind:value={plan.rationale} on:input={changed} disabled={busy} placeholder="What the evidence proves as a whole, and what it does not."></textarea></label>
  <div class="actions"><button class="primary" disabled={busy || editing} on:click={ondone}>Done</button><span class="small muted">{editing ? 'Keep or cancel the manual procedure first.' : 'Changes stay in the draft until you save the revision.'}</span></div>
</div>
<style>
  .editor { margin-top: 12px; }
  .evidence-line { display: flex; gap: 7px; align-items: center; }
  .evidence-line input { margin-top: 0; }
  .level-pick { width: auto; flex-shrink: 0; margin-top: 0; padding: 7px 8px; font-size: 12px; }
  .progress { margin: 0 0 12px; }
  .criteria { display: flex; flex-direction: column; gap: 8px; }
  .criterion { display: flex; gap: 12px; padding: 12px 14px; border: 1px solid var(--line); border-radius: 8px; background: var(--surface); }
  .done { border-color: #cfdcc9; }
  .mark { flex-shrink: 0; width: 20px; height: 20px; margin-top: 8px; border-radius: 50%; border: 1.5px solid var(--amber); color: white; font-size: 13px; font-weight: 700; display: grid; place-items: center; }
  .done .mark { background: var(--forest); border-color: var(--forest); }
  .body { flex: 1; min-width: 0; }
  .head { display: flex; gap: 8px; align-items: flex-start; }
  .statement { min-height: 0; margin: 0; font-weight: 600; field-sizing: content; }
  .remove { font-size: 20px; line-height: 1; padding: 6px 4px; color: var(--muted); }
  .remove:hover { color: var(--bad); }
  .evidence { margin: 10px 0 0 4px; padding-left: 12px; border-left: 2px solid var(--line); }
  .evidence input { margin-top: 4px; font-size: 13px; padding: 7px 10px; }
  .add { margin-top: 8px; padding: 4px 0; }
  .picker { margin: 10px 0 0; padding: 10px; background: var(--soft); border-radius: 7px; }
  .picker > input { margin: 0; font-size: 13px; }
  .choices { max-height: 260px; overflow: auto; margin: 8px 0; }
  .choice { display: block; width: 100%; text-align: left; margin-top: 5px; padding: 7px 10px; overflow-wrap: anywhere; white-space: normal; }
  .choice span { display: block; }
  .gap { min-height: 0; margin: 10px 0 0; font-size: 13px; padding: 7px 10px; field-sizing: content; color: var(--amber); border-color: #e6d9bf; background: #fffcf4; }
  .gap::placeholder { color: #b39a6b; }
  .next { display: flex; gap: 8px; margin-top: 8px; }
  .next select { width: auto; margin: 0; font-size: 12px; padding: 6px 8px; text-transform: uppercase; letter-spacing: .3px; }
  .next input { margin: 0; font-size: 13px; padding: 7px 10px; }
  .editor > button { margin-top: 10px; }
  .note { margin: 14px 0 0; }
  .actions { margin-top: 6px; }
</style>
