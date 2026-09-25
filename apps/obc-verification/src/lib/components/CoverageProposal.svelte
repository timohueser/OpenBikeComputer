<script lang="ts">
  import type { Catalog, CoverageProposalReview, Requirement } from '$lib/types';
  import { coverageChanges, coverageSummary, planCoveredCount } from '$lib/coverage';
  import { date } from './api';
  import CoverageChanges from './CoverageChanges.svelte';
  export let proposal: CoverageProposalReview;
  export let requirement: Requirement;
  export let catalog: Catalog;
  export let busy: boolean;
  export let onapprove: (proposal: CoverageProposalReview) => void;
  export let onreject: (id: string, feedback: string) => void;
  /** Bound by the parent so that Escape can close the reject box. */
  export let rejecting = false;
  let feedback = '';
  $: covered = planCoveredCount(proposal.plan);
  $: delta = coverageChanges(requirement, proposal.plan);
  $: deleted = delta.deletedProcedures;
  $: procedures = proposal.procedures ?? [];
  $: unchanged = proposal.plan.criteria.length - delta.added.length - delta.changed.length;
  $: lowers = lowering(coverageSummary(requirement), covered, proposal.plan.criteria.length);
  const rank = { uncovered: 0, partial: 1, covered: 2 } as const;
  const names = { uncovered: 'Not covered', partial: 'Partial', covered: 'Covered' } as const;
  /** Only an approved plan has a state to lose; an unassessed or unreviewed one does not. */
  function lowering(now: ReturnType<typeof coverageSummary>, count: number, total: number) {
    if (!(now.state in rank)) return '';
    const before = now.state as keyof typeof rank;
    const after = total > 0 && count === total ? 'covered' : count > 0 ? 'partial' : 'uncovered';
    if (rank[after] < rank[before]) return `This lowers coverage from ${names[before]} to ${names[after]}.`;
    if (count < now.covered) return `This lowers the covered criteria from ${now.covered} to ${count}.`;
    return '';
  }
  const focus = (node: HTMLElement) => node.focus();
</script>
<article class="proposal" aria-label={`Coverage proposal for ${proposal.requirementId}`}>
  <div class="row top">
    <h3>Proposal · {proposal.author}</h3>
    <span class="small muted">{date(proposal.createdAt)} · commit <code>{proposal.sourceSha.slice(0, 10)}</code> · <strong>{covered} of {proposal.plan.criteria.length}</strong> covered after approval</span>
  </div>
  <p class="delta small muted">{delta.added.length} new · {delta.changed.length} changed · {delta.removed.length} removed · {unchanged} unchanged{#if procedures.length} · {procedures.length} new {procedures.length === 1 ? 'procedure' : 'procedures'}{/if}</p>
  {#if lowers}<p class="alert warning">{lowers}</p>{/if}
  <CoverageChanges {requirement} plan={proposal.plan} {catalog} {procedures} />
  {#if proposal.conflict}<p class="alert error">{proposal.conflict}</p>{:else if proposal.stale}<p class="alert warning">{proposal.stale}</p>{/if}
  {#if deleted.length}<p class="small deletes">Approval deletes {deleted.length} manual {deleted.length === 1 ? 'procedure' : 'procedures'}, with {deleted.length === 1 ? 'its' : 'their'} steps and input files: {deleted.map(t => t.title).join(', ')}.</p>{/if}
  {#if rejecting}
    <label>Feedback for the agent<textarea rows={2} maxlength={5000} bind:value={feedback} use:focus placeholder="What should change?"></textarea></label>
    <div class="actions"><button class="danger-button" disabled={busy} on:click={() => onreject(proposal.id, feedback)}>Reject proposal</button><button disabled={busy} on:click={() => rejecting = false}>Keep reviewing <kbd>Esc</kbd></button></div>
  {:else}
    <div class="actions"><button class="primary" disabled={busy || !!proposal.conflict} on:click={() => onapprove(proposal)}>Approve &amp; next <kbd>A</kbd></button><button disabled={busy} on:click={() => rejecting = true}>Reject… <kbd>R</kbd></button><span class="small muted">Approval applies the evidence{procedures.length ? ` and the new ${procedures.length === 1 ? 'procedure' : 'procedures'}` : ''} to your draft and opens the next proposal. Save the revision to record your reviews together.</span></div>
  {/if}
</article>
<style>
  .proposal { padding: 16px 18px; margin: 12px 0 18px; border: 1px solid var(--coral); border-radius: 8px; background: var(--coral-bg); }
  .top h3 { font-size: 14px; }
  .delta { margin: 2px 0 8px; }
  .deletes { margin: 12px 0 0; color: var(--warn); }
  .actions { margin-top: 14px; }
  kbd { font: 600 10px/1 ui-monospace, SFMono-Regular, Consolas, monospace; padding: 2px 4px; border: 1px solid currentColor; border-radius: 3px; opacity: .7; }
</style>
