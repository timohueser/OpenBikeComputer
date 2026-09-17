<script lang="ts">
  import Markdown from './Markdown.svelte';
  export let value = '';
  export let label = 'Description';
  export let required = false;
  export let rows = 6;
  let preview = false;
</script>
<div class="field">
  <div class="row"><label class="field-label"><span>{label} <span class="muted small">· Markdown</span></span>
    <textarea class:visually-hidden={preview} bind:value {required} {rows} on:input></textarea>
  </label></div>
  <div class="editor-tools"><button type="button" class="text-button" on:click={() => preview = !preview}>{preview ? 'Continue editing' : 'Preview formatting'}</button></div>
  {#if preview}<div class="inset preview"><Markdown text={value || '*Nothing to preview yet.*'} /></div>{/if}
</div>
