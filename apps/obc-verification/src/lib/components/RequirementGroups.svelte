<script lang="ts">
  export let groups: { name: string; count: number }[];
  export let onrename: (name: string, replacement: string) => void;
  export let onremove: (name: string) => void;
  export let onclose: () => void;
  export let disabled = false;
  let editing: string | null = null;
  let replacement = '';
  let error = '';

  function rename(name: string) {
    if (disabled) return;
    const next = replacement.trim();
    error = '';
    if (!next) { error = 'Enter a group name. Use Remove group to move requirements to Ungrouped.'; return; }
    if (next !== name && groups.some(group => group.name === next)) { error = 'That group already exists. Choose a different name.'; return; }
    onrename(name, next); editing = null;
  }
</script>

<section class="panel">
  <div class="row"><h2>Manage groups</h2><button on:click={onclose}>Close</button></div>
  <p class="muted small">Group changes belong to your draft. Save a revision to keep them. Existing release candidates remain unchanged.</p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#each groups as group (group.name)}
    <div class="test">
      {#if editing === group.name}
        <form class="group-rename" on:submit|preventDefault={() => rename(group.name)}>
          <label>Group name<input {disabled} required maxlength={80} bind:value={replacement} /></label>
          <div class="actions"><button class="primary" {disabled}>Rename group</button><button type="button" on:click={() => { editing = null; error = ''; }}>Cancel</button></div>
        </form>
      {:else}
        <div class="row"><div><strong>{group.name}</strong><div class="small muted">{group.count} {group.count === 1 ? 'requirement' : 'requirements'}</div></div>
          <div class="actions"><button {disabled} on:click={() => { editing = group.name; replacement = group.name; error = ''; }}>Rename</button><button class="text-button danger" {disabled} on:click={() => {
            if (confirm(`Remove the group “${group.name}”? Its ${group.count} ${group.count === 1 ? 'requirement moves' : 'requirements move'} to Ungrouped. No requirements or tests are deleted.`)) { error = ''; onremove(group.name); }
          }}>Remove group</button></div>
        </div>
      {/if}
    </div>
  {:else}
    <p class="muted">No named groups yet. Edit a requirement and enter a Group name to create one.</p>
  {/each}
</section>
