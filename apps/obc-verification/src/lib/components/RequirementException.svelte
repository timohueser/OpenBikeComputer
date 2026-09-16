<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { Candidate, Requirement } from '$lib/types';
  import { api, date, message } from './api';

  export let candidate: Candidate;
  export let requirement: Requirement;
  export let admin = false;
  export let disabled = false;
  export let onchanged: (candidate: Candidate) => void | Promise<void>;
  export let ondirty: (requirementId: string, dirty: boolean) => void;
  export let onbusy: (busy: boolean) => void;
  let open = false;
  let reason = '';
  let removing = false;
  let busy = false;
  let error = '';
  $: exception = candidate.exceptions?.find(value => value.requirementId === requirement.id);
  $: ondirty(requirement.id, !!reason || removing);
  $: editable = admin && !disabled && !busy && !requirement.todo;
  onDestroy(() => ondirty(requirement.id, false));

  async function save(remove = false) {
    if (!editable || (!remove && (!reason.trim() || exception))) return;
    busy = true; onbusy(true); error = '';
    try {
      const path = `/api/candidates/${candidate.id}/exceptions`;
      const updated = remove
        ? await api<Candidate>(`${path}/${encodeURIComponent(requirement.id)}`, 'DELETE')
        : await api<Candidate>(path, 'POST', { requirementId: requirement.id, reason: reason.trim() });
      reason = ''; removing = false; open = false;
      await onchanged(updated);
    } catch (e) { error = message(e); }
    finally { busy = false; onbusy(false); }
  }
</script>

{#if exception}
  <div class="inset exception-record">
    <h4 class="warning">Accepted exception · this release only</h4>
    <p class="exception-reason">{exception.reason}</p>
    <p class="small muted">Accepted by {exception.author} · {date(exception.createdAt)}</p>
    {#if admin && !disabled}
      {#if removing}
        <p class="small warning">Remove this exception? This requirement will need passing verification before publication.</p>
        <div class="actions"><button class="danger" disabled={!editable} on:click={() => save(true)}>{busy ? 'Removing…' : 'Confirm removal'}</button><button disabled={busy} on:click={() => removing = false}>Keep exception</button></div>
      {:else}<button class="text-button danger" disabled={!editable} on:click={() => removing = true}>Remove exception</button>{/if}
    {/if}
  </div>
{/if}
{#if open}
  <form class="inset exception-record" on:submit|preventDefault={() => save()}>
    <h4>Accept an exception for this release</h4>
    <p class="small muted">Record the known gap and why this candidate can be released. This does not mark the requirement as verified or change its test results.</p>
    <label>Reason<textarea required maxlength={5000} rows={3} bind:value={reason} disabled={!editable}></textarea></label>
    {#if exception}<p class="warning small">An exception is already recorded. Remove it before accepting a different reason. Your draft is kept here.</p>{/if}
    {#if disabled}<p class="warning small">Exceptions cannot be changed while publication is frozen or another operation is in progress.</p>{/if}
    <div class="actions"><button class="primary" disabled={!editable || !reason.trim() || !!exception}>{busy ? 'Accepting…' : 'Accept exception for this release'}</button><button type="button" disabled={busy} on:click={() => { open = false; reason = ''; error = ''; }}>Discard exception draft</button></div>
  </form>
{:else if admin && !disabled && !exception && !requirement.todo}
  <button class="text-button" disabled={busy} on:click={() => open = true}>Accept exception for this release</button>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}
