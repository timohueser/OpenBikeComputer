<script lang="ts">
  import type { Catalog, CoverageProposalReview, Requirement } from '$lib/types';
  import { coverageChanges, planCoveredCount } from '$lib/coverage';
  import { date } from './api';
  import CoverageChanges from './CoverageChanges.svelte';
  export let proposal: CoverageProposalReview;
  export let requirement: Requirement;
  export let catalog: Catalog;
  export let busy: boolean;
  export let dirty: boolean;
  export let ondecide: (id: string, accept: boolean, feedback: string) => void;
  /** Bound by the parent so that Escape can close the reject box. */
  export let rejecting = false;
  let feedback = '';
  $: covered = planCoveredCount(proposal.plan);
  $: delta = coverageChanges(requirement, proposal.plan);
  $: deleted = delta.deletedProcedures;
  $: procedures = proposal.procedures ?? [];
  $: unchanged = proposal.plan.criteria.length - delta.added.length - delta.changed.length;
</script>
<article class="proposal" aria-label={`Coverage proposal for ${proposal.requirementId}`}>
  <div class="row top">
    <h3>Proposal · {proposal.author}</h3>
    <span class="small muted">{date(proposal.createdAt)} · commit <code>{proposal.sourceSha.slice(0, 10)}</code> · <strong>{covered} of {proposal.plan.criteria.length}</strong> covered after approval</span>
  </div>
  <p class="delta small muted">{delta.added.length} new · {delta.changed.length} changed · {delta.removed.length} removed · {unchanged} unchanged{#if procedures.length} · {procedures.length} new {procedures.length === 1 ? 'procedure' : 'procedures'}{/if}</p>
  <CoverageChanges {requirement} plan={proposal.plan} {catalog} {procedures} />
  {#if proposal.conflict}<p class="alert error">{proposal.conflict}</p>{:else if proposal.stale}<p class="alert warning">{proposal.stale}</p>{/if}
  {#if deleted.length}<p class="small deletes">Approval deletes {deleted.length} manual {deleted.length === 1 ? 'procedure' : 'procedures'}, with {deleted.length === 1 ? 'its' : 'their'} steps and input files: {deleted.map(t => t.title).join(', ')}.</p>{/if}
  {#if rejecting}
    <label>Feedback for the agent<textarea rows={2} maxlength={5000} bind:value={feedback} placeholder="What should change?"></textarea></label>
    <div class="actions"><button class="danger-button" disabled={busy} on:click={() => ondecide(proposal.id, false, feedback)}>Reject proposal</button><button disabled={busy} on:click={() => rejecting = false}>Keep reviewing</button></div>
  {:else}
    <div class="actions"><button class="primary" disabled={busy || dirty || !!proposal.conflict} on:click={() => ondecide(proposal.id, true, '')}>Approve</button><button disabled={busy} on:click={() => rejecting = true}>Reject…</button><span class="small muted">{dirty ? 'Save or discard your draft before approving.' : procedures.length ? 'Approval applies the evidence, creates the new procedure, and records your review.' : 'Approval records your review and applies the evidence above.'}</span></div>
  {/if}
</article>
<style>
  .proposal { padding: 16px 18px; margin: 12px 0 18px; border: 1px solid #e8d3c8; border-left: 4px solid var(--coral); border-radius: 8px; background: #fffaf7; }
  .top h3 { font-size: 14px; }
  .delta { margin: 2px 0 8px; }
  .deletes { margin: 12px 0 0; color: var(--amber); }
  .actions { margin-top: 14px; }
</style>
