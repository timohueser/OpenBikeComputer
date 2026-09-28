<script lang="ts">
    import type { Place } from '../../lib/planner/editor';

    let { landmark, onRide, onShow }: {
        landmark: Place;
        onRide: (landmark: Place) => void;
        onShow: (landmark: Place) => void;
    } = $props();
</script>

<p class="nearby">
    <span>Near the route: {landmark.label} ({landmark.description})</span>
    <span class="actions">
        <button type="button" class="planner-action" onclick={() => onRide(landmark)}>Ride over it</button>
        <button type="button" class="planner-action" onclick={() => onShow(landmark)}>Show</button>
    </span>
</p>

<style>
    .nearby {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px 12px;
        margin: 0;
        padding: 0 16px 8px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .actions {
        display: flex;
        gap: 8px;
        white-space: nowrap;
    }
</style>
