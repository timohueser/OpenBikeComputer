<script lang="ts">
  import type { AcceptanceCriterion, Catalog, CoverageEvidence, Requirement } from '$lib/types';
  import { criterionCovered, evidenceKey, evidenceTest } from '$lib/coverage';
  export let criterion: AcceptanceCriterion;
  export let requirement: Requirement;
  export let catalog: Catalog | undefined = undefined;
  /** When set, the card shows what changed since this earlier version of the criterion. */
  export let previous: AcceptanceCriterion | undefined = undefined;
  export let tag = '';
  export let removed = false;
  $: covered = !removed && criterionCovered(requirement, criterion);
  $: dropped = previous?.evidence.filter(e => !criterion.evidence.some(n => evidenceKey(n) === evidenceKey(e))) ?? [];
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
    {#each criterion.evidence as e}<p class="evidence wrap" class:added={isNew(e)} title={evidenceKey(e)}><strong>{title(e)}</strong>{#if isNew(e)}<span class="tag">new</span>{/if}<span class="muted">{' — '}{e.rationale}</span></p>{/each}
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
  .none { color: var(--amber); }
  .covered .evidence { border-left-color: #cfdcc9; }
  .tag { font-size: 10px; text-transform: uppercase; letter-spacing: .8px; color: var(--forest); font-weight: 700; margin-left: 4px; }
  .gap { margin: 8px 0 0; font-size: 13px; color: var(--amber); }
  del { color: var(--muted); }
</style>
