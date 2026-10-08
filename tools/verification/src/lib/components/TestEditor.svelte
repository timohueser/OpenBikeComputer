<script lang="ts">
  import type { ManualReason, VerificationTest } from '$lib/types';
  import MarkdownField from './MarkdownField.svelte';
  import Files from './Files.svelte';
  export let test: VerificationTest;
  export let onsave: (test: VerificationTest) => void;
  export let oncancel: () => void;
  let uploading = false;
</script>
<form class="inset" on:submit|preventDefault={() => onsave(test)}>
  <div class="row"><h2>Manual test</h2><span class="small muted">{test.id}</span></div>
  <label>Test name<input bind:value={test.title} required placeholder="For example: transfer a large route" /></label>
  <label>Manual test type
    <select bind:value={test.manualReason}>
      <option value={undefined}>Not set</option>
      <option value={'human' satisfies ManualReason}>Human check</option>
      <option value={'until-automated' satisfies ManualReason}>Until automated</option>
    </select>
  </label>
  <MarkdownField label="Steps" bind:value={test.steps} required />
  <MarkdownField label="Expected result" bind:value={test.expected} required rows={3} />
  <Files label="Input files" bind:files={test.inputs} editable onbusy={(busy) => uploading = busy} />
  <div class="actions section"><button class="primary" disabled={uploading}>Keep changes in draft</button><button type="button" on:click={oncancel}>Cancel</button></div>
</form>
