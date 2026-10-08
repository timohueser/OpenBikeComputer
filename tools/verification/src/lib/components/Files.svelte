<script lang="ts">
  import type { Attachment } from '$lib/types';
  import { api, message, size } from './api';
  export let files: Attachment[] = [];
  export let editable = false;
  export let label = 'Files';
  export let onbusy: (busy: boolean) => void = () => {};
  let uploading = false;
  let error = '';
  async function upload(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    uploading = true; onbusy(true); error = '';
    try {
      for (const file of Array.from(input.files || [])) {
        const body = new FormData(); body.append('file', file);
        files = [...files, await api<Attachment>('/api/files', 'POST', body)];
      }
    } catch (e) { error = message(e); }
    finally { uploading = false; onbusy(false); input.value = ''; }
  }
</script>
<div class="files">
  <h3>{label}</h3>
  {#each files as file, index}
    <div class="file row"><div><a href={'/api/files/' + encodeURIComponent(file.id)} download={file.name}>{file.name}</a><div class="muted small">{size(file.size)} · SHA-256 <code title={file.sha256}>{file.sha256.slice(0, 12)}</code></div></div>
      {#if editable}<button class="text-button danger" type="button" aria-label={'Remove ' + file.name} on:click={() => files = files.filter((_, i) => i !== index)}>Remove</button>{/if}
    </div>
  {/each}
  {#if !files.length}<p class="muted small">No files attached.</p>{/if}
  {#if editable}<label class="file-upload">{uploading ? 'Uploading…' : 'Attach files'}<input type="file" multiple disabled={uploading} on:change={upload} /></label><p class="muted small">Files are stored immediately. Save the form to attach them to this record.</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</div>
