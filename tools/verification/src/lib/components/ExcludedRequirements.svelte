<script lang="ts">
  import type { Requirement } from '$lib/types';
  import RequirementLabels from './RequirementLabels.svelte';
  export let requirements: Requirement[];
  $: excluded = requirements.filter(r => !r.active);
</script>
{#if excluded.length}
  <section class="inset excluded-requirements">
    <h3>Excluded from releases · {excluded.length}</h3>
    <p class="small">These requirements are outside verification for this candidate. Their labels and tests do not block publication.</p>
    {#each excluded as requirement}<div class="test"><strong>{requirement.id} · {requirement.title}</strong><div><RequirementLabels {requirement} /></div></div>{/each}
  </section>
{/if}
