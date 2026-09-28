<script lang="ts">
    import { dayColor } from '../../lib/planner/day-colors';
    import { profileHeights } from '../../lib/planner/profile-data';
    import { cumulative, type Coordinate, type Day } from '../../lib/planner/editor';
    let { coordinates, days, activeNight, window: area, onNight, dayLabels = {}, theme = 'light' }: {
        coordinates: Coordinate[]; days: Day[]; activeNight: number;
        window: {from:number;to:number;blocked:boolean} | null; onNight:(night:number)=>void; dayLabels?:Record<number,number>; theme?:'light'|'dark';
    } = $props();
    const total = $derived(cumulative(coordinates).at(-1) ?? 0);
    const heights = profileHeights;
    const line = heights.map((h,i)=>`${i/120*1000},${105-(h-350)/5}`).join(' ');
</script>
<section class="elevation" aria-label="Elevation profile">
    <div class="profile-title"><strong>Elevation</strong><span>Illustrative profile</span><span class="distance">{total.toFixed(1)} km</span></div>
    <div class="plot">
        <span class="height">700 m</span>
        <svg viewBox="0 0 1000 112" preserveAspectRatio="none" role="img" aria-label="Elevation profile. Shaded bands match the suggested overnight area on the map.">
            <path d="M0 25H1000M0 65H1000M0 105H1000" class="grid" />
            <polygon points={`0,112 ${line} 1000,112`} class="terrain" />
            {#if area && !area.blocked}<rect x={area.from*1000} width={(area.to-area.from)*1000} y="0" height="112" class="area" />{/if}
            {#each days as day}
                <svg x={day.from*1000} width={(day.to-day.from)*1000} height="112" viewBox={`${day.from*1000} 0 ${(day.to-day.from)*1000} 112`} preserveAspectRatio="none" overflow="hidden">
                    <polyline points={line} style:stroke={dayColor(dayLabels[day.number]??day.number,theme)}/>
                </svg>
            {/each}
            {#each days.slice(0,-1) as day}<line x1={day.to*1000} x2={day.to*1000} y1="0" y2="112" class:pinned={day.pinned} />{/each}
        </svg>
        {#each days.slice(0,-1) as day}<button style:background={dayColor(dayLabels[day.number]??day.number,theme)} style:color={theme==='dark'?'#201f17':'white'} class:active={activeNight===day.number} class:pinned={day.pinned} style:left={`${day.to*100}%`} onclick={()=>onNight(day.number)} aria-label={`Show day ${dayLabels[day.number]??day.number} on map`}>{dayLabels[day.number]??day.number}</button>{/each}
    </div>
    <div class="axis"><span>0 km</span><span>{Math.round(total/4)} km</span><span>{Math.round(total/2)} km</span><span>{Math.round(total*.75)} km</span><span>{Math.round(total)} km</span></div>
</section>
<style>
    .elevation{height:190px;flex:none;background:var(--panel);border-top:1px solid var(--line);padding:14px 24px 11px}
    .profile-title{display:flex;align-items:baseline;gap:12px;font-size:13px;margin-bottom:13px}
    .profile-title>span{color:var(--ink-faint);font-size:12px}.distance{margin-left:auto;font-variant-numeric:tabular-nums}
    .plot{height:106px;position:relative;margin:0 14px 0 35px}.plot>svg{width:100%;height:100%;overflow:visible}
    .height{position:absolute;left:-35px;top:15px;font-size:10px;color:var(--ink-faint)}
    .grid{fill:none;stroke:var(--line);stroke-width:1;vector-effect:non-scaling-stroke}.terrain{fill:color-mix(in srgb,var(--route) 10%,var(--panel))}
    polyline{stroke:var(--route);fill:none;stroke-width:1.7;vector-effect:non-scaling-stroke}
    line{stroke:var(--ink-faint);stroke-width:1;stroke-dasharray:3 4;vector-effect:non-scaling-stroke}line.pinned{stroke:var(--ink);stroke-dasharray:none}
    .area{fill:var(--amber);opacity:.25}
    button{position:absolute;top:77px;transform:translateX(-50%);width:23px;height:23px;display:grid;place-items:center;border:1px dashed var(--ink-faint);border-radius:50%;background:var(--panel);color:var(--ink);font:600 11px var(--sans);cursor:pointer}
    button.pinned{background:var(--ink);color:var(--panel);border-style:solid}button.active{outline:3px solid color-mix(in srgb,var(--amber) 55%,transparent);border-style:solid}button:focus-visible{outline:2px solid var(--rust);outline-offset:3px}
    .axis{display:flex;justify-content:space-between;margin:7px 9px 0 35px;font-size:10px;color:var(--ink-faint);font-variant-numeric:tabular-nums}
    @media(max-height:760px){.elevation{height:155px;padding-top:10px}.plot{height:80px}.profile-title{margin-bottom:8px}button{top:54px}}
</style>
