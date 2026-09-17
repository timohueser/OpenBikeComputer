<script lang="ts">
  import type { Catalog, CoverageProposalReview } from '$lib/types';
  import { date } from './api';
  import CoveragePlan from './CoveragePlan.svelte';
  import CoverageChanges from './CoverageChanges.svelte';
  import Markdown from './Markdown.svelte';
  export let proposals: CoverageProposalReview[];
  export let catalog: Catalog;
  export let busy: boolean;
  export let dirty: boolean;
  export let ondecide: (id: string, accept: boolean, feedback: string) => void;
  let feedback: Record<string, string> = {};

</script>

{#each proposals as p (p.id)}
  <article class="inset coverage-review" aria-label={`Coverage proposal for ${p.requirementId}`}>
    <div class="row"><h3>{p.requirementId} · {p.requirement?.title ?? 'Removed requirement'}</h3><span class="badge">{p.status}</span></div>
    {#if p.requirement}
      <div class="statement"><Markdown text={p.requirement.statement} /></div>
      <div class="assessment" class:partial={p.plan.conclusion === 'partial'}><strong>{p.plan.conclusion === 'complete' ? 'Proposed coverage: Complete' : 'Proposed coverage: Partial'}</strong><p class="small">{p.plan.conclusion === 'complete' ? 'Review whether the criteria capture the whole requirement and the evidence supports every criterion. Approval saves the plan and applies the test links and removals shown below.' : 'Approval records this plan and its gaps. The requirement will still block release even if every linked test passes.'}</p></div>
      {#if p.status === 'pending'}<CoverageChanges requirement={p.requirement} plan={p.plan} {catalog} /><details><summary>Full proposed coverage plan</summary><CoveragePlan plan={p.plan} requirement={p.requirement} {catalog} /></details>
      {:else}<CoveragePlan plan={p.plan} requirement={p.requirement} {catalog} />{/if}
    {/if}
    <p class="small muted wrap">Proposed by {p.author} · {date(p.createdAt)} · r{p.baseRevision}{p.agentToken ? ` · agent access issued by ${p.agentToken.issuedBy.name}` : ''}</p>
    {#if p.feedback}<p class="small wrap"><strong>Reviewer feedback:</strong> {p.feedback}</p>{/if}
    {#if p.decidedBy}<p class="small muted">Reviewed by {p.decidedBy} · {date(p.decidedAt!)}</p>{/if}
    {#if p.status === 'pending'}
      {#if p.conflict}<p class="alert warning">{p.conflict}</p>{/if}
      <label>Feedback to the agent <span class="muted">(optional)</span><textarea rows={2} maxlength={5000} bind:value={feedback[p.id]} placeholder="Explain what should change if you reject this plan."></textarea></label>
      <div class="actions"><button class="primary" disabled={busy || dirty || !!p.conflict} on:click={() => ondecide(p.id, true, feedback[p.id] ?? '')}>{p.plan.conclusion === 'complete' ? 'Approve changes — complete' : 'Approve changes — partial'}</button><button disabled={busy} on:click={() => ondecide(p.id, false, feedback[p.id] ?? '')}>Reject plan</button></div>
    {/if}
  </article>
{:else}<p class="muted">No coverage proposals match this view. An agent can propose new coverage or changes to an accepted plan.</p>{/each}

<style>
  .coverage-review { background: var(--surface); }
  .statement { margin-top: 15px; }
  .assessment { padding: 12px 15px; background: var(--soft); border-radius: 7px; margin-top: 16px; }
  .assessment.partial { background: #fff6e2; }
  .assessment p { margin-bottom: 0; }
</style>
