<script lang="ts">
  import type { Catalog, CoveragePlan, Requirement } from '$lib/types';
  import { evidenceTest } from '$lib/coverage';
  export let plan: CoveragePlan;
  export let requirement: Requirement;
  export let catalog: Catalog | undefined = undefined;
</script>

<p class="small muted">Assessed source <code>{plan.sourceSha.slice(0, 10)}</code> · {plan.criteria.length} acceptance criteria · agent assessment: <strong>{plan.conclusion}</strong></p>
<p class="wrap">{plan.rationale}</p>
<div class="criteria">
  {#each plan.criteria as criterion (criterion.id)}
    <article class="criterion">
      <div class="row"><h4>{criterion.id} · {criterion.statement}</h4><span class="badge" class:warning={!!criterion.gap || !criterion.evidence.length}>{criterion.gap || !criterion.evidence.length ? 'Gap remains' : 'Evidence mapped'}</span></div>
      {#each criterion.evidence as evidence}
        {@const test = evidenceTest(requirement, evidence)}
        {@const title = test?.title ?? catalog?.cases.find(c => c.id === evidence.caseId)?.name}
        <div class="evidence"><strong class="small wrap">{title ?? evidence.caseId ?? evidence.testId}</strong><p class="small wrap">{evidence.rationale}</p><details class="small"><summary>Test identity</summary><code class="wrap">{evidence.caseId ?? evidence.testId}</code></details></div>
      {:else}<p class="small warning">No test mapped to this criterion.</p>{/each}
      {#if criterion.gap}<p class="gap small wrap"><strong>Work still needed:</strong> {criterion.gap}</p>{/if}
    </article>
  {/each}
</div>

<style>
  .criterion { padding: 16px; margin: 10px 0; border: 1px solid var(--line); border-radius: 7px; }
  h4 { flex: 1; min-width: 160px; overflow-wrap: anywhere; }
  .evidence { border-left: 3px solid var(--line); padding-left: 12px; margin-top: 14px; }
  .evidence p { margin: 5px 0; }
  .gap { color: var(--amber); margin-bottom: 0; }
</style>
