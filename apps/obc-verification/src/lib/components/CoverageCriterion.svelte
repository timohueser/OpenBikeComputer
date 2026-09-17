<script lang="ts">
  import type { AcceptanceCriterion, Catalog, CoverageEvidence, Requirement, VerificationTest } from '$lib/types';
  import { criterionCovered, evidenceKey, evidenceTest, proposedCovered } from '$lib/coverage';
  import Markdown from './Markdown.svelte';
  import Files from './Files.svelte';
  export let criterion: AcceptanceCriterion;
  export let requirement: Requirement;
  export let catalog: Catalog | undefined = undefined;
  /** When set, the card shows what changed since this earlier version of the criterion. */
  export let previous: AcceptanceCriterion | undefined = undefined;
  export let tag = '';
  export let removed = false;
  /** A proposed criterion is judged on its own evidence, which is linked when the plan is approved. */
  export let proposed = false;
  /** The release view sets this to show each evidence test's outcome in one candidate. */
  export let result: ((test: VerificationTest) => { outcome: string; detail?: string; label?: string; disabled?: boolean; onrun?: () => void }) | undefined = undefined;
  $: covered = !removed && (proposed ? proposedCovered(criterion) : criterionCovered(requirement, criterion));
  /** A removed criterion takes all of its evidence with it. */
  $: kept = removed ? [] : criterion.evidence;
  $: dropped = removed ? criterion.evidence : previous?.evidence.filter(e => !criterion.evidence.some(n => evidenceKey(n) === evidenceKey(e))) ?? [];
  const title = (e: CoverageEvidence) => evidenceTest(requirement, e)?.title ?? catalog?.cases.find(c => c.id === e.caseId)?.name ?? evidenceKey(e);
  const isNew = (e: CoverageEvidence) => !!previous && !previous.evidence.some(p => evidenceKey(p) === evidenceKey(e));
</script>
<article class="criterion" class:covered class:removed>
  <span class="mark" aria-hidden="true">{covered ? '✓' : ''}</span>
  <div class="body">
    <div class="head">
      <p class="statement wrap">{#if previous && previous.statement !== criterion.statement}<del>{previous.statement}</del> {/if}{criterion.statement}</p>
      {#if tag}<span class="badge" class:success={tag === 'New'} class:warning={tag === 'Changed'} class:error={tag === 'Removed'}>{tag}</span>{/if}
    </div>
    {#each kept as e}
      {@const test = evidenceTest(requirement, e)}
      {@const found = test && result ? result(test) : undefined}
      <div class="evidence wrap" class:added={isNew(e)} title={evidenceKey(e)}>
        <p class="line"><strong>{title(e)}</strong>{#if isNew(e)}<span class="tag">new</span>{/if}<span class="muted">{' — '}{e.rationale}</span>{#if found}<span class="badge outcome" class:success={found.outcome === 'pass'} class:error={found.outcome === 'fail' || found.outcome === 'error'} class:warning={!['pass', 'fail', 'error'].includes(found.outcome)}>{found.outcome}</span>{#if found.onrun}<button class="run small" disabled={found.disabled} on:click={found.onrun}>{found.label}</button>{/if}{/if}</p>
        {#if test?.kind === 'manual'}<details class="small"><summary>Procedure</summary><Markdown text={test.steps || ''} /><h4>Expected result</h4><Markdown text={test.expected || ''} />{#if test.inputs.length}<Files files={test.inputs} label="Input files" />{/if}</details>{/if}
        {#if found?.detail}<details class="small"><summary>Test output</summary><pre class="wrap">{found.detail}</pre></details>{/if}
      </div>
    {/each}
    {#each dropped as e}<p class="evidence wrap dropped" title={evidenceKey(e)}><del><strong>{title(e)}</strong>{' — '}{e.rationale}</del></p>{/each}
    {#if !criterion.evidence.length}<p class="evidence none">No evidence yet</p>{/if}
    {#if criterion.gap}<p class="gap wrap"><strong>Gap:</strong> {criterion.gap}</p>{:else if previous?.gap}<p class="gap wrap"><del>Gap: {previous.gap}</del></p>{/if}
  </div>
</article>
<style>
  .criterion { display: flex; gap: 12px; padding: 12px 14px; border: 1px solid var(--line); border-radius: 8px; background: var(--surface); min-width: 0; }
  .covered { border-color: #cfdcc9; }
  .removed { opacity: .65; border-style: dashed; }
  .mark { flex-shrink: 0; width: 20px; height: 20px; margin-top: 1px; border-radius: 50%; border: 1.5px solid var(--amber); color: white; font-size: 13px; font-weight: 700; display: grid; place-items: center; }
  .covered .mark { background: var(--forest); border-color: var(--forest); }
  .removed .mark { border-color: var(--line); }
  .body { flex: 1; min-width: 0; }
  .head { display: flex; justify-content: space-between; gap: 10px; align-items: flex-start; }
  .statement { margin: 0; font-weight: 600; line-height: 1.45; }
  .evidence { margin: 6px 0 0; font-size: 13px; line-height: 1.5; padding-left: 12px; border-left: 2px solid var(--line); }
  .evidence.added { border-left-color: var(--forest); }
  .evidence.dropped { border-left-color: var(--bad); }
  .line { margin: 0; }
  .outcome { margin-left: 6px; }
  .run { margin-left: 8px; padding: 3px 9px; }
  .none { color: var(--amber); }
  .covered .evidence { border-left-color: #cfdcc9; }
  .tag { font-size: 10px; text-transform: uppercase; letter-spacing: .8px; color: var(--forest); font-weight: 700; margin-left: 4px; }
  .evidence details { margin-top: 2px; }
  .evidence h4 { margin: 8px 0 2px; }
  .gap { margin: 8px 0 0; font-size: 13px; color: var(--amber); }
  del { color: var(--muted); }
</style>
