<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import Callout from './Callout.svelte';
    import TrailMarker from './TrailMarker.svelte';
    import { networkLevels, networkScope, routeKindTitles, routeWebsite, type NetworkRoute, type OverlaySelection } from '../../lib/planner/route-overlays';
    let { selection, theme = 'light', onclose }: { selection: OverlaySelection; theme?: 'light' | 'dark'; onclose: () => void } = $props();
    // Relations with one name, number and network are one route to a rider: a route and its parts, or its two directions.
    const routes = $derived.by(() => {
        const groups = new Map<string, NetworkRoute[]>();
        for (const route of selection.routes ?? []) {
            const key = route.name || route.ref ? JSON.stringify([route.name, route.ref, route.network]) : String(route.id);
            groups.set(key, [...groups.get(key) ?? [], route]);
        }
        return [...groups.values()].map(members => ({ route: members[0], website: members.map(route => routeWebsite(route.website)).find(Boolean) }));
    });
    const levelColor = (rank: number) => networkLevels.find(level => level.rank === Math.min(rank, 3))?.[theme === 'dark' ? 'dark' : 'color'];
    const access: Record<string, { title: string; detail: string }> = {
        construction: { title: 'Under construction', detail: 'OSM maps this section as construction. The router excludes it.' },
        closed: { title: 'No access', detail: 'The mapped rules exclude this travel mode. This is not just a requirement to dismount.' },
        private: { title: 'Private access', detail: 'Permission is required. The router avoids this road where it can, and the route notes it.' },
        limited: { title: 'Limited access', detail: 'Access is limited, for example to destinations, farm traffic or permit holders. The router avoids this road where it can, and the route notes it.' },
        push: { title: 'Dismount and push', detail: 'Riding is not allowed here. You can push your bike; the router can include this as a walking section.' },
        no_bikes: { title: 'No bicycles', detail: 'Walking is allowed, but taking a bicycle through is restricted, including pushing.' },
        directional: { title: 'Directional access', detail: 'Access differs by direction. Check the mapped rules below.' },
        conditional: { title: 'Conditional access', detail: 'Access depends on conditions. The router does not evaluate them; the route notes them.' },
    };
    const restriction = $derived(access[selection.status ?? ''] ?? access.closed);
    const title = $derived(selection.kind === 'access'
        ? restriction.title
        : routeKindTitles[selection.kind]);
    function permission(values: boolean[] | undefined, bit: number) {
        if ((selection.conditional ?? 0) & bit) return 'Conditional';
        return values?.every(Boolean) ? 'Allowed' : values?.some(Boolean) ? 'One direction' : 'Not allowed';
    }
    const permissions = $derived([['Riding', permission(selection.riding, 1)], ['Pushing', permission(selection.pushing, 4)], ['Walking', permission(selection.walking, 2)]]);
</script>

<Callout label={title} onClose={onclose}>
    {#if selection.kind === 'access'}
        <header>
            <span class="glyph warn"><Icon name="lock" size={16} /></span>
            <div><h2>{title}</h2><p class="meta">{[selection.name, selection.ref].filter(Boolean).join(' · ') || 'Road or path'}</p></div>
        </header>
        <!-- The answer first: a word and a symbol for each way to pass. -->
        <dl class="permissions">
            {#each permissions as [way, answer] (way)}
                <div class={{ yes: answer === 'Allowed', no: answer === 'Not allowed' }}><dt>{way}</dt><dd>{#if answer === 'Allowed'}<Icon name="check" size={13} />{:else if answer === 'Not allowed'}<Icon name="close" size={13} />{/if}{answer}</dd></div>
            {/each}
        </dl>
        <p>{restriction.detail}</p>
        <details><summary class="sub"><span>Mapped access details</span><Icon name="down" size={15} /></summary><dl class="tags">{#each Object.entries(selection.tags ?? {}) as [key, value] (key)}<dt>{key}</dt><dd>{value}</dd>{/each}</dl></details>
        <p class="note"><a href={`https://www.openstreetmap.org/way/${selection.way}`} target="_blank" rel="noreferrer">OpenStreetMap</a> · Closure reports are not live.</p>
    {:else}
        <header>
            <span class="glyph"><Icon name="route" size={16} /></span>
            <div><h2>{title}</h2></div>
        </header>
        <!-- The badge shows the first of the numbers that OSM joins with ";". -->
        <ul>{#each routes as { route, website } (route.id)}
            {@const scope = networkScope(route)}
            <li>
                <span class="marker"><TrailMarker symbol={route.symbol} description={route.symbol_text ?? ''} ref={route.ref?.split(';')[0]} /></span>
                <span class="body">
                    <strong>{route.name || route.ref || 'Unnamed route'}</strong>
                    {#if scope || website}<small>
                        {#if scope}<i style:background={levelColor(route.rank)}></i>{scope}{/if}{#if scope && website}{' · '}{/if}
                        {#if website}<a href={website} target="_blank" rel="noreferrer">{new URL(website).hostname.replace(/^www\./, '')}</a>{/if}
                    </small>{/if}
                </span>
            </li>
        {/each}</ul>
    {/if}
</Callout>

<style>
    strong { display: block; font: 600 13px/1.4 var(--sans); overflow-wrap: anywhere; }
    p { color: var(--ink-soft); font-size: 12px; line-height: 1.5; }
    a { overflow-wrap: anywhere; }
    dl { margin: 0; overflow-wrap: anywhere; }
    dd { margin: 0; }
    .permissions { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 6px; }
    .permissions div { display: grid; gap: 2px; padding: 8px 10px; border-radius: 10px; background: var(--parchment-2); font-size: 12px; }
    .permissions dt { color: var(--ink-soft); font-size: 11px; }
    .permissions dd { display: flex; align-items: center; gap: 5px; font-weight: 600; }
    .yes dd { color: var(--water); }
    .no dd { color: var(--coral); }
    summary { list-style: none; }
    summary::-webkit-details-marker { display: none; }
    details[open] summary :global(svg) { transform: rotate(180deg); }
    .tags { display: grid; grid-template-columns: auto 1fr; gap: 3px 12px; padding: 8px 4px 0; font-size: 12px; }
    .tags dt { color: var(--ink-soft); }
    ul { display: grid; gap: 12px; margin: 0; padding: 0 4px; list-style: none; }
    li { display: flex; align-items: flex-start; gap: 10px; }
    .marker { flex: none; display: flex; min-width: 24px; }
    .body { flex: 1; min-width: 0; }
    small { display: block; margin-top: 2px; font: 400 13px/1.4 var(--sans); color: var(--ink-soft); }
    /* The colour of the route's network on the map. */
    small i { display: inline-block; width: 8px; height: 8px; margin-right: 6px; border-radius: 50%; }
</style>
