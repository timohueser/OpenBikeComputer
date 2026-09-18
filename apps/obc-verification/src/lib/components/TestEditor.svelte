<script lang="ts">
  import { TEST_KIND_NOTES, type ManualReason, type VerificationTest } from '$lib/types';
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
  <!-- Which of the two manual buckets this is. A human check is a permanent release duty; the other
       holds a place until a rig or an automated test can take it over. -->
  <label>Why a person runs it
    <select bind:value={test.manualReason}>
      <option value={undefined}>Not said yet</option>
      <option value={'human' satisfies ManualReason}>A person is the permanent answer</option>
      <option value={'until-automated' satisfies ManualReason}>Until an automated or rig test exists</option>
    </select>
  </label>
  <p class="small muted reason-note">{TEST_KIND_NOTES.find(n => n.key === test.manualReason)?.note ?? 'Say which, so a release can list the checks a person must run.'}</p>
  <MarkdownField label="Steps" bind:value={test.steps} required />
  <MarkdownField label="Expected result" bind:value={test.expected} required rows={3} />
  <Files label="Input files" bind:files={test.inputs} editable onbusy={(busy) => uploading = busy} />
  <div class="actions section"><button class="primary" disabled={uploading}>Keep changes in draft</button><button type="button" on:click={oncancel}>Cancel</button></div>
</form>
