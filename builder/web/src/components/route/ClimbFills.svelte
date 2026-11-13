<svelte:options namespace="svg" />

<script lang="ts">
    /** The stretches of the climbs of a profile, filled under its line: the area of the profile, cut to each stretch.
     * For an `svg` in the box of `profilePaths`, with `head` units of room over the box. */
    let { area, climbs, lit, head = 0 }: {
        /** The area path of `profilePaths`. */
        area: string;
        /** The ends of each stretch on the axis of the path, from 0 to 1, and its index. */
        climbs: { a: number; b: number; index: number }[];
        /** The index of the climb that is darker. */
        lit?: number;
        head?: number;
    } = $props();

    const uid = $props.id();
</script>

<defs>
    {#each climbs as climb (climb.index)}
        <clipPath id={`${uid}-climb-${climb.index}`}><rect x={climb.a * 1000} y={-head} width={(climb.b - climb.a) * 1000} height={105 + head} /></clipPath>
    {/each}
</defs>
{#each climbs as climb (climb.index)}
    <path class:lit={lit === climb.index} d={area} clip-path={`url(#${uid}-climb-${climb.index})`} />
{/each}

<style>
    path {
        fill: color-mix(in srgb, var(--ink-faint) 24%, var(--panel));
    }
    .lit {
        fill: color-mix(in srgb, var(--ink-faint) 38%, var(--panel));
    }
</style>
