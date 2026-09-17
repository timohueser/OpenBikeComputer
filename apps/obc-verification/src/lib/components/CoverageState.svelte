<script lang="ts">
  import type { Requirement } from '$lib/types';
  import { coverageReviewStatus, coverageStatus } from '$lib/coverage';
  export let requirement: Requirement;
  export let sourceSha: string | undefined = undefined;
  export let changed = false;
  $: coverage = coverageStatus(requirement);
  $: review = changed ? 'Needs review after saving' : coverageReviewStatus(requirement, sourceSha);
</script>
<div class="states small" aria-label="Coverage and review status">
  <span>Coverage: <strong class:warning={coverage !== 'Complete'}>{coverage}</strong></span>
  <span>Review: <strong class:warning={review !== 'Current'}>{review}</strong></span>
</div>
<style>.states { display: flex; flex-wrap: wrap; gap: 6px 18px; }</style>
