<script lang="ts">
    import type { Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import OpeningHours from './OpeningHours.svelte';
    import PlaceContent from './PlaceContent.svelte';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { websiteLabel, websiteLink, phoneNumbers, phoneLink } from '../../lib/planner/contact-links';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import type { Place } from '../../lib/planner/editor';

    /** The details of a place in a map popup: its symbol, its name or the `title` in its place, its kind, its article,
     * note, contacts and opening hours. */
    let { place, title, aside, error }: {
        place: Place;
        title?: Snippet;
        /** Added to the kind line. */
        aside?: string;
        error?: string;
    } = $props();

    const website = $derived(websiteLink(place.website));
    const phones = $derived(phoneNumbers(place.phone));
</script>

<header class="place-heading">
    <span class="place-symbol"><Icon path={placeCategories[place.category].icon} size={19} /></span>
    <div>
        {#if title}{@render title()}{:else}<h2>{place.label}</h2>{/if}
        <p class="meta kind">{[place.placeKind ? kindLabel(place.placeKind) : placeCategories[place.category].label, place.locality, aside].filter(Boolean).join(' · ')}</p>
    </div>
</header>
{#if place.content}<PlaceContent content={place.content} name={place.label} />{/if}
{#if place.description && place.description !== placeCategories[place.category].label}<p class="place-note">{place.description}</p>{/if}
{#if website || place.website || phones.length}
    <ul class="contacts">
        {#if website}<li><Icon name="globe" size={15} /><a href={website} target="_blank" rel="noopener noreferrer">{websiteLabel(website)}</a></li>
        {:else if place.website}<li><Icon name="globe" size={15} /><span>{place.website}</span></li>{/if}
        {#each phones as phone}
            {@const link = phoneLink(phone)}
            <li><Icon name="phone" size={15} />{#if link}<a href={link}>{phone}</a>{:else}<span>{phone}</span>{/if}</li>
        {/each}
    </ul>
{/if}
{#if error}<p class="meta" role="status">{error}</p>{/if}
{#if place.openingHours || ['shop','food','pharmacy','hotel','bike'].includes(place.category)}<OpeningHours value={place.openingHours} />{/if}

<style>
    .place-heading { display: flex; gap: 12px; align-items: flex-start; }
    .place-heading div { flex: 1; min-width: 0; }
    .place-symbol { display: grid; place-items: center; width: 36px; height: 36px; flex: none; border-radius: 50%; color: var(--place-disc); background: color-mix(in srgb, var(--place-disc) 10%, var(--panel)); }
    /* A kind of the search data is in small letters. */
    .kind::first-letter { text-transform: uppercase; }
    .place-note { line-height: 1.5; white-space: pre-wrap; overflow-wrap: anywhere; }
    .contacts { display: grid; gap: 6px; margin: 0; padding: 0; list-style: none; font-variant-numeric: tabular-nums; }
    .contacts li { display: flex; align-items: center; gap: 8px; min-width: 0; }
    .contacts :global(svg) { flex: none; color: var(--ink-soft); }
    /* A long address stays on its line. */
    .contacts a, .contacts span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .contacts a:focus-visible { outline: 2px solid var(--link); outline-offset: 3px; border-radius: 2px; }
</style>
