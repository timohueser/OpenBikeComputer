<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, message } from './api';

  interface HistoryPreview {
    baseRevision: number;
    revisionCount: number;
    protectedRevisionCount: number;
    requirementCount: number;
    testCount: number;
  }

  export let workspaceDirty = false;
  export let onsuccess: () => void;
  let preview: HistoryPreview | null = null;
  let loading = true;
  export let busy = false;
  let clearCurrent = false;
  let confirmation = '';
  let discardDrafts = false;
  let error = '';
  $: phrase = clearCurrent ? 'START FRESH' : 'CLEAR HISTORY';
  $: ready = !!preview && !loading && !busy && confirmation === phrase && (!workspaceDirty || discardDrafts);

  async function fetchPreview() {
    loading = true; preview = null;
    try { preview = await api<HistoryPreview>('/api/admin/history'); }
    finally { loading = false; }
  }

  async function refresh() {
    error = ''; confirmation = '';
    try { await fetchPreview(); }
    catch (e) { error = message(e); }
  }

  async function clearHistory() {
    if (!ready || !preview) return;
    busy = true; error = '';
    try {
      await api('/api/admin/history', 'POST', { baseRevision: preview.baseRevision, clearCurrent, confirmation });
      onsuccess();
    } catch (e) {
      confirmation = '';
      error = message(e);
      if (e instanceof ApiError && e.status === 409) {
        try { await fetchPreview(); error += ' Review the updated counts and type the confirmation again.'; }
        catch (refreshError) { error += ` ${message(refreshError)}`; }
      }
    } finally { busy = false; }
  }

  onMount(refresh);
</script>

<div class="row"><h2>Clear requirements history</h2><button disabled={busy || loading} on:click={refresh}>Refresh preview</button></div>
<p class="muted">Remove old requirements revisions and coverage proposals. Your current requirements and tests stay in a new revision unless you choose to clear them below.</p>
{#if error}<div class="alert error" role="alert">{error}</div>{/if}
{#if loading}<p class="muted" role="status">Loading the current history…</p>
{:else if preview}
  <div class="inset">
    <div class="review-row"><span>Requirement revisions to remove</span><strong>{Math.max(0, preview.revisionCount - preview.protectedRevisionCount)}</strong></div>
    <div class="review-row"><span>Revisions kept for release candidates</span><strong>{preview.protectedRevisionCount}</strong></div>
    <div class="review-row"><span>Current requirements / linked tests</span><strong>{preview.requirementCount} / {preview.testCount}</strong></div>
    <p class="small muted">Preview based on requirements r{preview.baseRevision}.</p>
  </div>
  <p class="small muted">Release candidates, published releases, their evidence, attachments, accounts, and the automated test catalogue are retained. This action does not delete server backups.</p>
  <form on:submit|preventDefault={clearHistory}>
    <fieldset disabled={busy}>
      <label class="check"><input type="checkbox" bind:checked={clearCurrent} on:change={() => { confirmation = ''; discardDrafts = false; }} />Also clear the current requirements and tests — start fresh</label>
      <div class="alert warning">{clearCurrent ? `This will remove ${preview.requirementCount} current ${preview.requirementCount === 1 ? 'requirement' : 'requirements'} and ${preview.testCount} linked ${preview.testCount === 1 ? 'test' : 'tests'} from the working set. Release candidates keep their original copies.` : 'The current saved requirements and tests will be kept. The removed history and coverage proposals cannot be restored through this application.'}</div>
      {#if clearCurrent}<p class="small error">This cannot be undone through the application.</p>{/if}
      {#if workspaceDirty}<label class="check"><input type="checkbox" bind:checked={discardDrafts} />I understand that my unsaved requirement, release, and password changes will be discarded when the workspace reloads.</label>{/if}
      <label>Type <code>{phrase}</code> to confirm<input bind:value={confirmation} autocomplete="off" autocapitalize="off" spellcheck={false} aria-label="History confirmation" /></label>
      <button class="danger-button" disabled={!ready}>{busy ? 'Clearing history…' : clearCurrent ? 'Clear history and start fresh' : 'Clear history, keep current requirements'}</button>
    </fieldset>
  </form>
{/if}
