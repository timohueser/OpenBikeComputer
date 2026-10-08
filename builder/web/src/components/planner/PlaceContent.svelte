<script lang="ts">
    import { onlinePhoto, articleVariant, type PlaceContent } from '../../lib/planner/place-content';

    let { content, name }: { content: PlaceContent; name: string } = $props();
    const article = $derived(articleVariant(content));
    let failed = $state(false);
    let photoUrl = $state<string>();
    let loading = $state(false);
    $effect(() => {
        const photo = content.photo, abort = new AbortController();
        photoUrl = undefined; failed = false; loading = Boolean(photo?.online_url);
        if (photo) onlinePhoto(photo, abort.signal).then(url => {
            if (!abort.signal.aborted) photoUrl = url;
        }).catch(() => {}).finally(() => { if (!abort.signal.aborted) loading = false; });
        return () => abort.abort();
    });
</script>

<section class="place-content" aria-label={`About ${name}`}>
    {#if article}
        <div lang={article.language}>
            {#each article.text_pages as page}<p>{page}</p>{/each}
        </div>
        <p class="credit"><a href={article.attribution.source_url} target="_blank" rel="noopener noreferrer">Wikipedia contributors</a>
            · <a href={article.attribution.license_url} target="_blank" rel="noopener noreferrer">Article licence</a></p>
    {/if}
    {#if content.photo}
        {#if photoUrl && !failed}
            <img src={photoUrl} alt={name} loading="lazy" referrerpolicy="no-referrer" onerror={() => failed = true} />
        {:else if loading}
            <p class="credit" role="status">Loading photo…</p>
        {:else}
            <p class="credit">Photo unavailable. Photos need an internet connection.</p>
        {/if}
        <p class="credit">{content.photo.credit.filter(Boolean).join(' · ')}
            · <a href={content.photo.source_url} target="_blank" rel="noopener noreferrer">Photo source</a>
            · <a href={content.photo.license_url} target="_blank" rel="noopener noreferrer">Photo licence</a></p>
    {/if}
</section>

<style>
    .place-content { margin-block: 12px; }
    p { margin-block: 8px; white-space: pre-line; }
    img { display: block; width: 100%; max-height: 240px; object-fit: contain; margin-block: 12px 8px; }
    .credit { font-size: 11px; line-height: 1.5; color: var(--ink-soft); }
    a { color: inherit; text-underline-offset: 2px; }
    a:focus-visible { outline: 2px solid var(--ochre); outline-offset: 2px; }
</style>
