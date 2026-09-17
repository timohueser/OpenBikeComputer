<script lang="ts">
  import type { ProposalReview } from '$lib/types';
  import { date } from './api';
  export let suggestions: ProposalReview[];
  export let busy: boolean;
  export let dirty: boolean;
  export let ondecide: (id: string, accept: boolean) => void;
</script>
{#each suggestions as p (p.id)}
  <div class="suggestion" aria-label={`Link suggestion for ${p.requirementId}`}>
    <div class="row">
      <p class="wrap"><strong>{p.action === 'add' ? 'Link' : 'Unlink'} {p.test?.name ?? p.caseId}</strong><span class="muted">{' — '}{p.reason}</span><br /><span class="small muted">{p.author} · {date(p.createdAt)}</span></p>
      <div class="actions"><button class="primary" disabled={busy || dirty || !!p.conflict} on:click={() => ondecide(p.id, true)}>Approve</button><button disabled={busy} on:click={() => ondecide(p.id, false)}>Reject</button></div>
    </div>
    {#if p.conflict}<p class="small warning">{p.conflict}</p>{/if}
  </div>
{/each}
<style>
  .suggestion { padding: 12px 16px; margin: 10px 0; border: 1px solid #e8d3c8; border-left: 4px solid var(--coral); border-radius: 8px; background: #fffaf7; }
  .suggestion p { margin: 0; }
  .suggestion .small { margin-top: 6px; }
</style>
