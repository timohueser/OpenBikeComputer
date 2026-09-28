<script lang="ts">
    import type { Place } from '../../lib/planner/editor';

    let { landmark, onRide, onShow }: {
        landmark: Place;
        onRide: (landmark: Place) => void;
        onShow: (landmark: Place) => void;
    } = $props();
</script>

<p class="nearby">
    Near the route: {landmark.label} ({landmark.description})
    · <button type="button" onclick={() => onRide(landmark)}>Ride over it</button>
    · <button type="button" onclick={() => onShow(landmark)}>Show</button>
</p>

<style>
    .nearby {
        margin: 0;
        padding: 0 16px 12px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    button {
        padding: 0;
        border: 0;
        background: none;
        color: var(--forest);
        font: inherit;
        font-weight: 600;
        text-decoration: underline;
        text-underline-offset: 3px;
        cursor: pointer;
    }
</style>
