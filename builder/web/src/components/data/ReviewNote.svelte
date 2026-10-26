<script lang="ts">
    import { prepareHash } from "../../lib/data/screens";
    import { prepareOf, type Note } from "../../lib/data/shapes";

    // A note of a review. A note that names `obc data source prepare` links the prepare screen of
    // this page, for the target `to` and the region of the review.
    let { note, to, region, tone }: { note: Note; to: string; region: string; tone: "missing" | "stale" } = $props();

    const prepare = $derived(prepareOf(note.text));
</script>

<p class={tone}>
    <b>{note.layer}</b> · {note.text}
    {#if prepare}
        <a href={prepareHash(prepare.id, to, { flag: "--region", value: prepare.region ?? region })}>Prepare {prepare.id} on this page</a>
    {/if}
</p>
