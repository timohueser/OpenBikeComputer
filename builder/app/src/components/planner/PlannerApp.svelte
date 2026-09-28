<script lang="ts">
    import { onMount, tick } from 'svelte';
    import PlannerMap from './PlannerMap.svelte';
    import Icon from './PlannerIcon.svelte';
    import Profile from './PlannerProfile.svelte';
    import Query from './PlannerQuery.svelte';
    import Resize from './PanelResize.svelte';
    import { ridingProfiles, type BikeType } from '../../lib/planner/riding-profiles';
    import { reorderPoint, routeStops, anchorProgress, addRestDay, removeRestDay, itineraryDays, applyBudget, coordinateAt, cumulative, initialTrip, kilometres, nearestProgress, nightOrderConflicts, overnightWindow, pinNight, places, routeCoordinates, routeSlice, TripHistory, tripDays, addPointNear, appendPoint, dayOverTarget, insertPoint, overnightCandidates, setDrawnLeg, setLegMode, setSplit, type Coordinate, type Place, type RoutePoint, type Trip, type Day, type LegMode, type OvernightCandidate } from '../../lib/planner/editor';
    import { poiKinds } from '../../lib/planner/poi-kinds';
    import { dayColor } from '../../lib/planner/day-colors';
    import { profileAscent } from '../../lib/planner/profile-data';
    import type { PlannerQueryValue } from '../../lib/planner/query';
    import type { MapPoi, MapPoint, MapSegment } from '../../lib/planner/map-types';

    type Panel = 'trip' | 'search' | 'budget' | 'route' | 'checks' | 'setup';
    type EditableKind = 'via'|'pass'|'waypoint'|'night'|'marker';
    type CalloutKind = 'point' | 'place' | 'dayend' | 'add' | 'leg';
    const pointTypes: {kind:EditableKind;label:string;icon:string}[] = [{kind:'via',label:'Shape',icon:'route'},{kind:'waypoint',label:'Visit',icon:'flag'},{kind:'night',label:'Sleep',icon:'camp'},{kind:'marker',label:'Marker',icon:'pin'}];
    const defaultLabels: Record<EditableKind, string> = { via: 'Shaping point', pass: 'Pass here', waypoint: 'Visit', night: 'Overnight spot', marker: 'Marker' };
    const legModes: { mode: LegMode; label: string }[] = [{ mode: 'routed', label: 'Routed' }, { mode: 'straight', label: 'Straight' }, { mode: 'drawn', label: 'Drawn' }];
    let trip = $state<Trip>(initialTrip());
    let panel = $state<Panel>('trip');
    let night = $state(1);
    let expandedDay = $state<number|null>(1);
    let restEditing = $state<number|null>(null);
    let sideWidth = $state(360);
    let profileHeight = $state(190);
    let viewportHeight = $state(900);
    let viewportWidth = $state(1200);
    const maxProfile = $derived(Math.max(140,Math.min(340,viewportHeight-400)));
    const maxSide = $derived(Math.max(320,Math.min(460,viewportWidth-540)));
    const bike = $derived(trip.bike??'touring');
    const stops = $derived(routeStops(trip));
    let sleepDay = $state(1);
    let selectedId = $state<string|null>(null);
    let pending = $state<Coordinate|null>(null);
    let pendingSource = $state<string|null>(null);
    let renaming = $state(false);
    let changingOvernight = $state(false);
    let adding = $state<EditableKind|null>(null);
    // Where an add or leg callout opened; a leg callout also names its leg.
    let spot = $state<{ coordinate: Coordinate; legEndId?: string } | null>(null);
    let drawing = $state<string | null>(null);
    let tilePlace = $state<Place | null>(null);
    let theme = $state<'light'|'dark'>('light');
    let hillshade = $state(true);
    let contours = $state(true);
    let showRoute = $state(true);
    let query = $state('');
    let searchValue = $state<PlannerQueryValue>({text:'',category:'all',day:null,within:null});
    let budget = $state<Trip['budget']>('days');
    let target = $state(3);
    let limit = $state(50);
    let message = $state('Example route and places');
    let saveStatus = $state('');
    let map: PlannerMap|undefined;
    const history = new TripHistory();
    let revision = $state(0);
    const canUndo = $derived.by(()=>{void revision;return history.canUndo;});
    const canRedo = $derived.by(()=>{void revision;return history.canRedo;});
    const coordinates = $derived(routeCoordinates(trip));
    const total = $derived(cumulative(coordinates).at(-1)!);
    const days = $derived(tripDays(trip));
    const multi = $derived(trip.mode !== 'route');
    const availableTypes = $derived(pointTypes.filter(t=>multi||t.kind!=='night'));
    // One stretch per leg and day, so the map can tell legs apart and colour days.
    const segments = $derived.by(() => {
        const length = stops.at(-1)!.distance || 1;
        return stops.slice(1).flatMap((stop, i) => days.flatMap((day): MapSegment[] => {
            const from = Math.max(day.from, stops[i].distance / length);
            const to = Math.min(day.to, stop.distance / length);
            if (to <= from) return [];
            const color = dayColor(dayLabels[day.number] ?? day.number, theme);
            return [{ coordinates: routeSlice(coordinates, from, to), color, legEndId: stop.point.id, leg: stop.point.leg ?? 'routed' }];
        }));
    });
    const longestDay = $derived(Math.max(1,...days.map(d=>d.distance)));
    const itinerary = $derived(itineraryDays(trip));
    const dayLabels = $derived(Object.fromEntries(itinerary.filter(d=>!d.rest).map(d=>[d.ridingNumber,d.number])));
    const overTargets = $derived(days.map(day => dayOverTarget(trip, day, profileAscent(day.from, day.to))));
    const warnings = $derived(multi ? days.filter((_, i) => overTargets[i].km > 0) : []);
    const conflicts = $derived(multi?nightOrderConflicts(trip):[]);
    const warningCount = $derived(warnings.length+conflicts.length);
    const activeDay = $derived(days[Math.min(night-1,days.length-1)]);
    const area = $derived(night<days.length?overnightWindow(trip,night):null);
    const overnightContext = $derived(multi&&expandedDay!==null&&panel==='trip'&&night<days.length&&(!activeDay?.pinned||changingOvernight)&&!adding);
    const highlighted = $derived(overnightContext&&area&&!area.blocked?routeSlice(coordinates,area.from,area.to):[]);
    const candidates = $derived(multi ? overnightCandidates(trip, night) : []);
    const suggested = $derived(candidates.map(candidate => candidate.place));
    const searchDay = $derived(searchValue.day===null?null:itinerary.find(d=>d.number===searchValue.day));
    const results = $derived(places.filter(p=>searchValue.category==='all'||p.category===searchValue.category).filter(p=>searchValue.within===null||kilometres(p.coordinate,coordinateAt(coordinates,searchDay?.to??nearestProgress(coordinates,p.coordinate)))<=searchValue.within).sort((a,b)=>searchDay?Math.abs(nearestProgress(coordinates,a.coordinate)-searchDay.to)-Math.abs(nearestProgress(coordinates,b.coordinate)-searchDay.to):a.progress-b.progress));
    const visiblePlaces = $derived(panel==='search'?results:overnightContext?suggested:[]);
    const selectedPlace = $derived(places.find(p=>p.id===selectedId) ?? (tilePlace?.id===selectedId ? tilePlace : undefined));
    const selectedPoint = $derived(trip.points.find(p=>p.id===selectedId));
    const previewCoordinate = $derived(selectedId==='pending'?pending:selectedPlace?.coordinate);
    const previewDay = $derived(previewCoordinate&&sleepDay<days.length?tripDays(pinNight(trip,sleepDay,previewCoordinate,'Preview'))[sleepDay-1]:null);
    const previewDistance = $derived(previewDay?.distance??null);
    const mapPoints = $derived.by(() => {
        const pins: MapPoint[] = trip.points.map(p => ({
            ...p,
            kind: !multi && p.kind === 'night' ? 'waypoint' : p.kind,
            color: multi && p.kind === 'night' ? dayColor(dayLabels[p.night!] ?? p.night!, theme) : undefined,
            markerLabel: multi && p.kind === 'night' ? String(dayLabels[p.night!]) : undefined,
            fixed: p.kind === 'night' || (['waypoint', 'detour'].includes(p.kind) && p.label !== defaultLabels.waypoint),
        }));
        for (const day of multi ? days.slice(0, -1) : []) {
            if (day.pinned) continue;
            const label = dayLabels[day.number] ?? day.number;
            pins.push({ id: `dayend-${day.number}`, kind: 'dayend', night: day.number, coordinate: coordinateAt(coordinates, day.to), label: `Day ${label} ends here for now`, markerLabel: String(label), color: dayColor(label, theme) });
        }
        const shown = selectedPlace && places.includes(selectedPlace) && !visiblePlaces.includes(selectedPlace) ? [...visiblePlaces, selectedPlace] : visiblePlaces;
        for (const p of shown) {
            if (p.id === selectedId || !pins.some(pin => pin.coordinate[0] === p.coordinate[0] && pin.coordinate[1] === p.coordinate[1])) {
                pins.push({ ...p, appearance: p.category === 'hotel' || p.category === 'camp' ? p.category : undefined });
            }
        }
        if (pending) pins.push({ id: 'pending', coordinate: pending, label: 'Overnight spot', kind: 'place', appearance: 'suggested' });
        return pins;
    });
    const calloutKind = $derived<CalloutKind | null>(
        selectedId === 'add' || selectedId === 'leg' ? selectedId
        : selectedId?.startsWith('dayend-') ? 'dayend'
        : selectedPoint ? 'point'
        : selectedPlace || selectedId === 'pending' ? 'place'
        : null,
    );
    const calloutCoordinate = $derived.by(() => {
        if (calloutKind === 'add' || calloutKind === 'leg') return spot?.coordinate ?? null;
        if (calloutKind === 'place') return previewCoordinate ?? null;
        if (!showRoute && selectedPoint?.kind !== 'marker') return null;
        return mapPoints.find(p => p.id === selectedId)?.coordinate ?? null;
    });
    const legMode = $derived(trip.points.find(p => p.id === spot?.legEndId)?.leg ?? 'routed');
    onMount(()=>{
        try{const raw=localStorage.getItem('obc-planner-lab-v1');const saved=raw?JSON.parse(raw):null;if(saved&&Array.isArray(saved.points)&&saved.points.every((p:RoutePoint)=>Array.isArray(p.coordinate)&&p.coordinate.length===2&&p.coordinate.every(Number.isFinite))&&Number.isFinite(saved.days)&&saved.days>=1&&saved.days<=14&&Number.isFinite(saved.limit))trip=saved;if(!saved)panel='setup';syncBudget();}catch{saveStatus='Browser storage unavailable';}
    });
    $effect(()=>{document.documentElement.dataset.theme=theme;});
    function syncBudget(){budget=trip.budget;target=trip.target;limit=trip.limit;}
    function save(){try{localStorage.setItem('obc-planner-lab-v1',JSON.stringify(trip));saveStatus='Saved locally';}catch{saveStatus='Could not save locally';}}
    function commit(next:Trip,description:string){trip=history.commit($state.snapshot(trip),next);revision++;message=description;save();}
    function clearSelection() {
        selectedId = null;
        pending = null;
        pendingSource = null;
        renaming = false;
        spot = null;
    }
    function undo(){trip=history.undo($state.snapshot(trip));revision++;clearSelection();night=Math.min(night,trip.days);syncBudget();save();message='Change undone';}
    function redo(){trip=history.redo($state.snapshot(trip));revision++;clearSelection();syncBudget();save();message='Change restored';}
    function openPanel(next:Panel){panel=next;adding=null;changingOvernight=false;clearSelection();if(next==='budget')syncBudget();}
    async function focusDay(number:number){const collapse=expandedDay===number&&panel==='trip';night=number;sleepDay=number;expandedDay=collapse?null:number;openPanel('trip');if(collapse)return;await tick();map?.fitCoordinates(routeSlice(coordinates,Math.max(0,activeDay.from-.04),Math.min(1,activeDay.to+.04)));}
    function selectPlace(place:Place){pendingSource=null;sleepDay=night;selectedId=place.id;pending=null;renaming=false;map?.showPlace(place.coordinate,12);}
    function selectPoint(id: string) {
        if (id === 'pending') return;
        clearSelection();
        selectedId = id;
        const number = id.startsWith('dayend-') ? Number(id.slice('dayend-'.length)) : trip.points.find(p => p.id === id)?.night;
        if (number && multi) {
            night = number;
            sleepDay = number;
            expandedDay = number;
            panel = 'trip';
            changingOvernight = false;
        }
    }
    function inspectPoint(p:RoutePoint){selectPoint(p.id);map?.showPlace(p.coordinate,12);}
    function emptyClick(coordinate: Coordinate) {
        if (adding) {
            const kind = adding;
            adding = null;
            if (kind === 'night') {
                sleepDay = night;
                pending = coordinate;
                selectedId = 'pending';
            } else {
                addPoint(coordinate, kind);
            }
            return;
        }
        if (selectedId) {
            clearSelection();
            return;
        }
        spot = { coordinate };
        selectedId = 'add';
    }
    function addHere(kind: EditableKind) {
        const coordinate = spot!.coordinate;
        if (kind !== 'night') {
            addPoint(coordinate, kind);
            return;
        }
        clearSelection();
        sleepDay = night;
        pending = coordinate;
        selectedId = 'pending';
    }
    function legClick(legEndId: string, coordinate: Coordinate) {
        clearSelection();
        adding = null;
        spot = { coordinate, legEndId };
        selectedId = 'leg';
    }
    function setLeg(mode: LegMode) {
        const legEndId = spot!.legEndId!;
        if (mode === 'drawn') {
            clearSelection();
            drawing = legEndId;
            return;
        }
        commit(setLegMode($state.snapshot(trip), legEndId, mode), mode === 'straight' ? 'Leg set to a straight line' : 'Leg set to routed');
    }
    function insert(legEndId: string, coordinate: Coordinate) {
        clearSelection();
        commit(insertPoint($state.snapshot(trip), legEndId, coordinate), 'Shaping point inserted');
    }
    function drawn(legEndId: string, coordinates: Coordinate[]) {
        drawing = null;
        commit(setDrawnLeg($state.snapshot(trip), legEndId, coordinates), 'Leg drawn');
    }
    function moveDayEnd(number: number, progress: number) {
        commit(setSplit($state.snapshot(trip), number, progress), 'Day end moved');
    }
    function selectTilePlace(poi: MapPoi) {
        const kind = poiKinds[poi.kind];
        clearSelection();
        adding = null;
        tilePlace = { id: `poi-${poi.id}`, kind: 'place', label: poi.label, coordinate: poi.coordinate, progress: anchorProgress(poi.coordinate), category: kind.category, description: kind.label };
        sleepDay = night;
        selectedId = tilePlace.id;
    }
    function stayHere(){
        if(!previewCoordinate)return;
        const source=trip.points.find(p=>p.id===pendingSource);
        const label=selectedPlace?.label??(source&&!['via','pass'].includes(source.kind)?source.label:'Overnight spot');
        let next=$state.snapshot(trip);
        if(pendingSource){next.points=next.points.filter(p=>p.id!==pendingSource);next.routeOrder=next.routeOrder?.filter(id=>id!==`night-${sleepDay}`||id===pendingSource).map(id=>id===pendingSource?`night-${sleepDay}`:id);}
        pendingSource=null;
        if(sleepDay>=next.days){next.days=sleepDay+1;if(next.budget==='days')next.target++;}
        night=sleepDay;expandedDay=night;
        commit(pinNight(next,night,[...previewCoordinate],label),'Overnight pinned');panel='trip';changingOvernight=false;pending=null;selectedId=`night-${night}`;
    }
    function newPoint(coordinate: Coordinate, kind: EditableKind, label?: string): RoutePoint {
        return { id: crypto.randomUUID(), coordinate: [...coordinate], label: label ?? defaultLabels[kind], kind, progress: anchorProgress(coordinate) };
    }
    function addPoint(coordinate: Coordinate, kind: EditableKind = 'via') {
        const point = newPoint(coordinate, kind);
        commit(appendPoint($state.snapshot(trip), point), 'Point added');
        clearSelection();
        selectedId = point.id;
    }
    function addVisit(place: Place) {
        const point = newPoint(place.coordinate, 'waypoint', place.label);
        commit(addPointNear($state.snapshot(trip), point), 'Visit added');
        clearSelection();
        selectedId = point.id;
    }
    function movePoint(id:string,coordinate:Coordinate){const p=trip.points.find(p=>p.id===id);if(!p)return;if(p.kind==='night')commit(pinNight($state.snapshot(trip),p.night!,coordinate,p.label),'Overnight moved');else commit({...$state.snapshot(trip),points:$state.snapshot(trip.points).map(p=>p.id===id?{...p,coordinate,progress:['start','finish'].includes(p.kind)?p.progress:anchorProgress(coordinate)}:p)},'Point moved');}
    function removePoint() {
        if (!selectedPoint || ['start', 'finish'].includes(selectedPoint.kind)) return;
        const next = $state.snapshot(trip);
        next.points = next.points.filter(p => p.id !== selectedId);
        next.routeOrder = next.routeOrder?.filter(id => id !== selectedId);
        commit(next, 'Point removed');
        clearSelection();
    }
    function rename(label:string){if(!selectedPoint||!label.trim())return;commit({...$state.snapshot(trip),points:$state.snapshot(trip.points).map(p=>p.id===selectedId?{...p,label:label.trim()}:p)},'Point renamed');renaming=false;}
    function changeKind(kind:EditableKind|'detour'){
        if(!selectedPoint)return;
        if(kind==='night'){sleepDay=night;pendingSource=selectedPoint.id;pending=[...selectedPoint.coordinate];selectedId='pending';return;}
        const p=selectedPoint;let next=$state.snapshot(trip);
        const id=p.kind==='night'?crypto.randomUUID():p.id;next.points=next.points.map(x=>x.id===p.id?{...x,id,kind,night:undefined,label:kind==='via'?'Shaping point':x.kind==='via'?(kind==='waypoint'?'Visit':kind==='marker'?'Marker':'Pass here'):x.label}:x);selectedId=id;next.routeOrder=next.routeOrder?.map(old=>old===p.id?id:old);
        commit(next,'Point type updated');renaming=false;
    }
    function setMode(mode:'route'|'trip'){commit({...$state.snapshot(trip),mode},mode==='route'?'Single route':'Multi-day trip');night=1;sleepDay=1;expandedDay=1;openPanel('trip');}
    function apply(){if(!Number.isFinite(target)||target<1||!Number.isFinite(limit)||limit<0)return;commit(multi?applyBudget($state.snapshot(trip),budget,target,limit):{...$state.snapshot(trip),limit},'Day budget updated');night=Math.min(night,trip.days);openPanel('trip');}
    function search(value:PlannerQueryValue){searchValue=value;if(value.day!==null){const d=itinerary.find(d=>d.number===value.day);if(d)night=d.ridingNumber;}openPanel('search');}
    function candidateDay(p:Place){return night<days.length?tripDays(pinNight(trip,night,p.coordinate,p.label))[night-1]:null;}
    function nameRest(index:number,name:string){const names=(trip.restAfter??[]).map((_,i)=>i===index?name.trim():trip.restNames?.[i]??'');commit({...$state.snapshot(trip),restNames:names},'Rest day named');restEditing=null;}
    function chooseBike(value:BikeType){commit({...$state.snapshot(trip),bike:value,preset:ridingProfiles[value].presets[0]},'Bike preference saved · routing is mocked');}
    function startLabel(day:Day){return day.number===1?trip.points.find(p=>p.kind==='start')!.label:days[day.number-2]?.pinned?.label??'Open overnight';}
    function endLabel(day:Day){return day.number===days.length?trip.points.find(p=>p.kind==='finish')!.label:day.pinned?.label??'Choose overnight';}
    function duration(hours:number){return `${Math.floor(hours)}h ${Math.round(hours%1*60)}m`;}
    function dayStops(day:Day){return trip.points.filter(p=>['pass','waypoint','detour'].includes(p.kind)&&nearestProgress(coordinates,p.coordinate)>day.from&&nearestProgress(coordinates,p.coordinate)<=day.to).sort((a,b)=>a.progress-b.progress);}
    function keyboard(e: KeyboardEvent) {
        if ((e.target as HTMLElement)?.closest('input,select,textarea')) return;
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'z') {
            e.preventDefault();
            if (e.shiftKey) redo();
            else undo();
        }
        if (e.key === 'Escape') {
            clearSelection();
            adding = null;
            drawing = null;
        }
    }
</script>

<svelte:window onkeydown={keyboard} bind:innerHeight={viewportHeight} bind:innerWidth={viewportWidth}/>
<div class="planner-shell" style:--side-width={`${Math.min(sideWidth,maxSide)}px`}>
    <header class="site-header"><a class="brand" href="/"><img src="/brand/app-icon.svg" alt=""/><span>OpenBikeComputer</span></a><nav aria-label="Preview navigation"><a href="/map-study.html">Map study</a><span>Planner</span></nav><button class="theme" aria-label={theme==='light'?'Use dark theme':'Use light theme'} onclick={()=>theme=theme==='light'?'dark':'light'}><Icon name={theme==='light'?'moon':'sun'}/></button></header>
    <div class="trip-bar">
        <div class="trip-name"><strong>{trip.points.find(p=>p.kind==='start')?.label} → {trip.points.find(p=>p.kind==='finish')?.label}</strong><button class="mode-button" onclick={()=>openPanel('setup')}>{multi?'Multi-day trip':'Single route'}<Icon name="down" size={12}/></button></div>
        <div class="ride-controls"><label>Bike<select aria-label="Bike type" value={bike} onchange={e=>chooseBike(e.currentTarget.value as BikeType)}>{#each Object.entries(ridingProfiles) as [id,profile]}<option value={id}>{profile.label}</option>{/each}</select></label><label>Preset<select aria-label="Routing preset" value={trip.preset??'Balanced'} onchange={e=>commit({...$state.snapshot(trip),preset:e.currentTarget.value},'Preset saved · routing is mocked')}>{#each ridingProfiles[bike].presets as preset}<option>{preset}</option>{/each}</select></label></div>
        <nav class="planning-tools" aria-label="Planning tools"><button class:active={panel==='trip'} onclick={()=>openPanel('trip')}><Icon name="route"/>Itinerary</button><button class:active={panel==='route'} onclick={()=>openPanel('route')}>Route alternatives</button>{#if multi}<button class:active={panel==='budget'} onclick={()=>openPanel('budget')}><Icon name="sliders"/>{multi?'Day budget':'Daily limit'}</button><button class:active={panel==='checks'} onclick={()=>openPanel('checks')}><Icon name={warningCount?'warning':'check'}/>Checks{#if warningCount}<b class="count">{warningCount}</b>{/if}</button>{/if}</nav>
        <div class="edit-actions"><button class="icon-button" disabled={!canUndo} onclick={undo} aria-label="Undo"><Icon name="undo"/></button><button class="icon-button" disabled={!canRedo} onclick={redo} aria-label="Redo"><Icon name="redo"/></button><details class="add-menu"><summary class:active={adding}><Icon name="plus"/>Add point<Icon name="down" size={13}/></summary><div>{#each availableTypes as type}<button onclick={e=>{adding=type.kind;sleepDay=night;clearSelection();e.currentTarget.closest('details')!.open=false;}}><Icon name={type.icon}/>{type.label}</button>{/each}</div></details></div>
    </div>
    <main>
        <aside class="planner-pane" aria-label="Trip planning">
            <div class="search-dock"><Query bind:text={query} days={itinerary.length} onSearch={search} onClear={()=>openPanel('trip')}/></div>
            <div class="trip-summary"><div><strong>{total.toFixed(1)}<small>km</small></strong><span>Distance</span></div><div><strong>{profileAscent()}<small>m</small></strong><span>Ascent</span></div><div><strong>{duration(total/15)}</strong><span>Riding time</span></div><div class="summary-bottom"><span>{multi?`${itinerary.length} days`:'Single route'}{#if multi&&trip.restAfter?.length} · {trip.restAfter.length} rest{/if}</span>{#if multi}<button onclick={()=>openPanel('budget')}>{trip.limit?`${trip.limit} km / day limit`:'No distance limit'}<Icon name="sliders" size={13}/></button>{/if}</div></div>
            <div class="pane-scroll">
                {#if panel==='trip'&&!multi}
                    <div class="detail-panel"><h1>Route</h1><p class="fixture-note">From start · climb is illustrative</p>{#each stops as {point,distance}, index}<div class="route-stop"><button class="stop-detail" onclick={()=>inspectPoint(point)}><Icon name={point.kind==='via'?'route':point.kind==='start'?'pin':'flag'} size={15}/><span><strong>{point.kind==='via'?'Shaping point':point.label}</strong><small>{distance.toFixed(1)} km · ↑ {profileAscent(0,total?distance/total:0)} m</small></span></button>{#if index>0&&index<stops.length-1}<div class="reorder"><button aria-label={`Move ${point.label} earlier`} disabled={index===1} onclick={()=>commit(reorderPoint($state.snapshot(trip),point.id,-1),'Stop moved earlier')}><Icon name="up" size={14}/></button><button aria-label={`Move ${point.label} later`} disabled={index===stops.length-2} onclick={()=>commit(reorderPoint($state.snapshot(trip),point.id,1),'Stop moved later')}><Icon name="down" size={14}/></button></div>{:else}<small>{index===0?'Start':'Finish'}</small>{/if}</div>{/each}<button class="secondary route-add" onclick={()=>{adding='via';clearSelection();}}>Add shaping point<Icon name="plus" size={14}/></button></div>
                {:else if panel==='trip'}
                    <div class="pane-heading"><h1>Itinerary</h1></div>
                    <div class="itinerary">
                        {#each itinerary as day}
                            {#if day.rest}<div class="rest-row"><span class="day-number rest"><Icon name="pause" size={14}/></span><div>{#if restEditing===day.restIndex}<input aria-label={`Name rest day ${day.number}`} value={trip.restNames?.[day.restIndex!]??''} placeholder="Rest day name" onblur={e=>nameRest(day.restIndex!,e.currentTarget.value)} onkeydown={e=>{if(e.key==='Enter')e.currentTarget.blur();if(e.key==='Escape')restEditing=null;}}/>{:else}<button class="rest-name" aria-label={`Name rest day ${day.number}`} onclick={()=>restEditing=day.restIndex!}><strong>Day {day.number} · {trip.restNames?.[day.restIndex!]||'Rest'}</strong><Icon name="pencil" size={12}/></button>{/if}<small>{day.pinned?.label??'Open overnight'}</small></div><button class="icon-button" aria-label={`Remove rest day ${day.number}`} onclick={()=>commit(removeRestDay($state.snapshot(trip),day.restIndex!),'Rest day removed')}><Icon name="close" size={15}/></button></div>
                            {:else}
                                <section class="day-section" style:--day-color={dayColor(day.number,theme)} class:expanded={expandedDay===day.ridingNumber}>
                                    <button class="day-heading" onclick={()=>focusDay(day.ridingNumber)} aria-expanded={expandedDay===day.ridingNumber} aria-label={`Day ${day.number}: ${endLabel({...day,number:day.ridingNumber})}`}>
                                        <span class="day-number">{day.number}</span><span class="day-title"><strong>{startLabel({...day,number:day.ridingNumber})} <span>→</span> {endLabel({...day,number:day.ridingNumber})}</strong><small>{duration(day.hours)} <span>·</span> ↑ {profileAscent(day.from,day.to)} m</small><span class="length-track"><span style:width={`${day.distance/longestDay*100}%`}></span></span></span><span class="day-distance" class:over={warnings.some(d=>d.number===day.ridingNumber)}>{day.distance.toFixed(1)}<small>km</small></span><Icon name={expandedDay===day.ridingNumber?'down':'chevron'} size={13}/>
                                    </button>
                                    {#if expandedDay===day.ridingNumber}
                                        <div class="day-content">
                                            <div class="stop-line"><span class="stop-dot"></span><span>{startLabel({...day,number:day.ridingNumber})}</span><small>Start</small></div>
                                            {#each dayStops(day) as point}<button class="stop-line" onclick={()=>inspectPoint(point)}><Icon name={point.kind==='pass'?'pin':'flag'} size={14}/><span>{point.label}</span><small>{point.kind==='detour'?'Out & back':point.kind==='pass'?'Pass':'Visit'}</small></button>{/each}
                                            <button class="stop-line end" onclick={()=>{if(day.pinned)inspectPoint(day.pinned);else if(day.ridingNumber===days.length)inspectPoint(trip.points.find(p=>p.kind==='finish')!);else map?.fitCoordinates(routeSlice(coordinates,Math.max(0,(area?.from??day.to)-.05),Math.min(1,(area?.to??day.to)+.05)));}}><Icon name={day.ridingNumber===days.length?'flag':'camp'} size={15}/><span>{endLabel({...day,number:day.ridingNumber})}</span>{#if day.pinned}<Icon name="pin" size={13}/>{/if}</button>
                                            {#if day.ridingNumber<days.length&&(!day.pinned||changingOvernight)}
                                                {#if area?.blocked&&!previewCoordinate}<div class="inline-warning"><Icon name="warning" size={14}/><span>No overnight area fits the limit</span><button onclick={()=>openPanel('budget')}>Edit limit</button></div>{/if}<div class="stay-list">{#each candidates as candidate}{@render stayRow(candidate.place, candidate)}{/each}</div>
                                                <button class="map-pick" onclick={()=>{clearSelection();adding='night';}}><Icon name="plus" size={14}/>Pick overnight on map</button>
                                            {/if}
                                            {#if warnings.some(d=>d.number===day.ridingNumber)&&!changingOvernight&&!previewCoordinate}<div class="inline-warning"><Icon name="warning" size={14}/><span>{(day.distance-trip.limit).toFixed(1)} km over limit</span><button onclick={()=>openPanel('budget')}>Edit limit</button></div>{/if}
                                            {#if day.ridingNumber<days.length}<div class="day-actions">{#if day.pinned}<button onclick={()=>{changingOvernight=!changingOvernight;clearSelection();}}>{changingOvernight?'Keep overnight':'Change overnight'}</button>{/if}</div>{/if}
                                        </div>
                                    {/if}
                                </section>
                                {#if day.ridingNumber<days.length}<button class="between-days" onclick={()=>commit(addRestDay($state.snapshot(trip),day.ridingNumber),'Rest day added')}><Icon name="plus" size={12}/>Add rest day</button>{/if}
                            {/if}
                        {/each}
                    </div>
                {:else}
                    <div class="detail-panel"><button class="back" onclick={()=>openPanel('trip')}><Icon name="back" size={14}/>Itinerary</button>
                        {#if panel==='setup'}<h1>Plan type</h1><button class="route-option" class:chosen={!multi} onclick={()=>setMode('route')}><Icon name="route"/><span><strong>Single route</strong><small>One ride, with visits and shaping points</small></span></button><button class="route-option" class:chosen={multi} onclick={()=>setMode('trip')}><Icon name="camp"/><span><strong>Multi-day trip</strong><small>Overnight stays, day budgets and rest days</small></span></button><p class="form-help">Switch at any time. The route and saved day plan stay intact.</p>
                        {:else if panel==='search'}<div class="pane-heading"><h1>Places</h1><span>{results.length} results</span></div>{#if results.length}<div class="stay-list">{#each results as place}{@render stayRow(place)}{/each}</div>{:else}<p>No example places match these constraints.</p>{/if}<p class="fixture-note">Example places · no live availability</p>
                        {:else if panel==='budget'}<h1>Day budget</h1><form class="budget-form" onsubmit={e=>{e.preventDefault();apply();}}>{#if multi}<label for="budget">Plan by</label><select id="budget" bind:value={budget} onchange={()=>target=budget==='days'?itinerary.length:budget==='distance'?50:4}><option value="days">Days available</option><option value="distance">Distance per day</option><option value="hours">Riding hours per day</option></select><label for="target">{budget==='days'?'Number of days':budget==='distance'?'Distance · km':'Riding hours'}</label><input id="target" type="number" min="1" max={budget==='days'?14:undefined} required bind:value={target}/>{/if}<h2>Daily limits</h2><label for="limit">Maximum distance · km</label><input id="limit" type="number" min="0" required bind:value={limit}/><p class="form-help">Longer days appear in Checks. Use 0 for no limit.</p><button class="primary" type="submit">Apply<Icon name="check"/></button></form>
                        {:else if panel==='route'}<h1>Route alternatives</h1>{#each [['valley','Along the valley','Canal and Doubs corridor'],['direct','More direct','Fewer bends']] as [id,title,description]}<button class="route-option" class:chosen={trip.variant===id} onclick={()=>commit({...$state.snapshot(trip),variant:id as Trip['variant']},'Route alternative changed')}><span class="radio-mark">{#if trip.variant===id}●{/if}</span><span><strong>{title}</strong><small>{description}</small></span><b>{cumulative(routeCoordinates({...trip,variant:id as Trip['variant']})).at(-1)!.toFixed(1)}<small>km</small></b></button>{/each}<p class="fixture-note">Example geometry · pinned places stay fixed</p>
                        {:else}<h1>Checks <span class="muted">{warningCount}</span></h1>{#each conflicts as [before,after]}<div class="check-row"><Icon name="warning"/><div><h2>Overnight order</h2><p>Day {dayLabels[after.night!]} ends before day {dayLabels[before.night!]}.</p><button onclick={()=>focusDay(after.night!)}>Show day {dayLabels[after.night!]}</button></div></div>{/each}{#each warnings as day}<div class="check-row"><Icon name="warning"/><div><h2>Day {dayLabels[day.number]} · {(day.distance-trip.limit).toFixed(1)} km over</h2><p>{day.distance.toFixed(1)} km planned · {trip.limit} km limit</p><div class="inline-actions"><button onclick={()=>focusDay(day.number)}>Show day</button><button onclick={()=>openPanel('budget')}>Edit limit</button></div></div></div>{/each}{#if !warningCount}<p class="all-clear"><Icon name="check"/>No distance or overnight-order warnings</p>{/if}{/if}
                    </div>
                {/if}
            </div>
        </aside>
        <Resize value={Math.min(sideWidth,maxSide)} min={320} max={maxSide} axis="x" label="Sidebar width" onResize={value=>sideWidth=value}/>
        <section class="geography" aria-label="Map and elevation">
            <div class="map-area">
                <PlannerMap
                    bind:this={map} {segments} {coordinates} points={mapPoints} {selectedId} callout={calloutCoordinate} {drawing}
                    {theme} {hillshade} {contours} {showRoute} highlightedCoordinates={highlighted} pickMode={!!adding}
                    onEmptyClick={emptyClick} onPointSelect={selectPoint} onPointMove={movePoint} onDayEndDrag={moveDayEnd}
                    onLegClick={legClick} onInsert={insert} onDrawn={drawn} onPoiClick={selectTilePlace}
                >
                    {#snippet popup()}
                        <div class="point-popup">
                            <button class="popup-close" onclick={clearSelection} aria-label="Close"><Icon name="close" size={14}/></button>
                            {#if calloutKind === 'add'}
                                <h2>Add point here</h2>
                                <div class="point-types" aria-label="Point type">
                                    {#each availableTypes.filter(type => type.kind !== 'marker') as type}
                                        <button onclick={() => addHere(type.kind)}><Icon name={type.icon} size={15}/>{type.label}</button>
                                    {/each}
                                </div>
                                <button class="add-marker" onclick={() => addHere('marker')}><Icon name="pin" size={13}/>Marker</button>
                            {:else if calloutKind === 'leg'}
                                <h2>This leg</h2>
                                <div class="reach-options" aria-label="Leg mode">
                                    {#each legModes as option}
                                        <button class:chosen={legMode === option.mode} aria-pressed={legMode === option.mode} onclick={() => setLeg(option.mode)}>{option.label}</button>
                                    {/each}
                                </div>
                                <button class="secondary" onclick={() => insert(spot!.legEndId!, spot!.coordinate)}>Insert point here</button>
                            {:else if calloutKind === 'dayend'}
                                <h2>Day {dayLabels[night]} ends here for now</h2>
                                <div class="stay-list">{#each candidates as candidate}{@render stayRow(candidate.place, candidate)}{/each}</div>
                                <button class="map-pick" onclick={() => { clearSelection(); adding = 'night'; }}><Icon name="plus" size={14}/>Pick another spot on the map</button>
                                <p class="popup-hint">Drag the marker along the route to move the day end.</p>
                            {:else if calloutKind === 'place'}
                                <h2>{selectedPlace?.label??'Overnight spot'}</h2>
                                {#if !multi||(selectedPlace&&selectedPlace.category!=='hotel'&&selectedPlace.category!=='camp')}<button class="primary" onclick={()=>addVisit(selectedPlace!)}>Add visit<Icon name="plus"/></button>{:else}<label class="sleep-target" for="sleep-day">End of day<select id="sleep-day" bind:value={sleepDay}>{#each days as day}<option value={day.number}>Day {dayLabels[day.number]}{day.pinned?` · replace ${day.pinned.label}`:day.number===days.length?' · add overnight':''}</option>{/each}</select></label>{#if previewDistance!==null}<div class="popup-distance"><span>Day {dayLabels[sleepDay]}</span><strong>{previewDistance.toFixed(1)} km · ↑ {profileAscent(previewDay?.from??0,previewDay?.to??0)} m</strong></div>{#if trip.limit>0&&previewDistance>trip.limit}<div class="inline-warning"><Icon name="warning" size={14}/><span>{(previewDistance-trip.limit).toFixed(1)} km over</span><button onclick={()=>openPanel('budget')}>Edit limit</button></div>{/if}{/if}<button class="primary" onclick={stayHere}>{days[sleepDay-1]?.pinned?'Replace overnight':'Stay here'}<Icon name="check"/></button>{#if selectedPlace}<button class="secondary" onclick={()=>addVisit(selectedPlace!)}>Add as visit</button>{/if}{/if}
                            {:else if selectedPoint}<div class="popup-title"><h2>{selectedPoint.kind==='via'?'Shaping point':selectedPoint.label}</h2>{#if selectedPoint.kind!=='via'}<button class="icon-button" aria-label="Rename point" onclick={()=>renaming=!renaming}><Icon name="pencil" size={14}/></button>{/if}</div>{#if renaming}<input aria-label="Point name" value={selectedPoint.label} onchange={e=>rename(e.currentTarget.value)} onkeydown={e=>{if(e.key==='Enter')rename(e.currentTarget.value);}}/>{/if}
                                {#if !['start','finish'].includes(selectedPoint.kind)}<div class="point-types" aria-label="Point type">{#each availableTypes as type}<button class:chosen={selectedPoint.kind===type.kind||(type.kind==='waypoint'&&selectedPoint.kind==='detour')} onclick={()=>changeKind(type.kind)}><Icon name={type.icon} size={15}/>{type.label}</button>{/each}</div>{#if ['waypoint','detour'].includes(selectedPoint.kind)}<div class="reach-options"><button class:chosen={selectedPoint.kind==='waypoint'} onclick={()=>changeKind('waypoint')}>Through</button><button class:chosen={selectedPoint.kind==='detour'} onclick={()=>changeKind('detour')}>Out and back</button></div>{/if}<button class="remove" onclick={removePoint}>Remove point</button>{/if}
                            {/if}
                        </div>
                    {/snippet}
                </PlannerMap>
                <div class="map-controls" aria-label="Map controls"><div class="control-group"><button onclick={()=>map?.zoomBy(1)} aria-label="Zoom in"><Icon name="plus"/></button><button onclick={()=>map?.zoomBy(-1)} aria-label="Zoom out"><Icon name="minus"/></button></div><div class="control-group"><button onclick={()=>map?.fitRoute()} aria-label="Show whole route"><Icon name="fit"/></button><button class:chosen={showRoute} aria-label={showRoute?'Hide route':'Show route'} aria-pressed={showRoute} onclick={()=>showRoute=!showRoute}><Icon name="eye"/></button></div><details class="layer-menu"><summary aria-label="Map layers"><Icon name="layers"/></summary><div><strong>Map layers</strong><label><input type="checkbox" bind:checked={hillshade}/>Relief</label><label><input type="checkbox" bind:checked={contours}/>Contours</label></div></details></div>
            </div>
            <Resize value={Math.min(profileHeight,maxProfile)} min={130} max={maxProfile} axis="y" label="Elevation height" onResize={value=>profileHeight=value}/>
            <Profile height={Math.min(profileHeight,maxProfile)} {coordinates} {days} {dayLabels} {theme} activeNight={expandedDay??0} window={overnightContext?area:null} onNight={focusDay}/>
            <div class="status-line" role="status">{#if adding}<strong>Place {pointTypes.find(t=>t.kind===adding)?.label.toLowerCase()} on map</strong><button onclick={()=>adding=null}>Cancel</button>{:else if drawing}<strong>Draw the leg on the map · Esc cancels</strong><button onclick={()=>drawing=null}>Cancel</button>{:else}<span>{message}</span>{/if}<span class="status-meta">{saveStatus}{saveStatus?' · ':''}Routing & elevation mocked</span></div>
        </section>
    </main>
</div>

{#snippet stayRow(place: Place, prediction: Pick<OvernightCandidate, 'distance' | 'from' | 'to'> | null = candidateDay(place))}
    <button class="stay-row" class:selected={selectedId===place.id} onclick={()=>selectPlace(place)}>
        <Icon name={place.category==='hotel'?'hotel':place.category==='camp'?'camp':'water'} size={17}/>
        <span><strong>{place.label}</strong><small>{place.category==='hotel'?'Hotel':place.category==='camp'?'Campsite':'Water'}</small></span>
        {#if place.category!=='water'&&prediction}<b>{prediction.distance.toFixed(1)} km<small>↑ {profileAscent(prediction.from,prediction.to)} m · day {dayLabels[night]}</small></b>{/if}
        <Icon name="chevron" size={12}/>
    </button>
{/snippet}

<style>
    :global(*){box-sizing:border-box;scrollbar-width:thin;scrollbar-color:var(--wood) var(--panel)}
    :global(body){margin:0;background:var(--parchment);color:var(--ink);font:14px/1.4 var(--sans);-webkit-font-smoothing:antialiased}
    :global(button),:global(input),:global(select){font:inherit}
    :global(button){cursor:pointer}:global(button:disabled){opacity:.35;cursor:default}
    :global(button:focus-visible),:global(input:focus-visible),:global(select:focus-visible),:global(a:focus-visible),:global(summary:focus-visible){outline:2px solid var(--forest);outline-offset:3px}
    button{color:inherit;border:0;background:none;padding:0}button:hover{color:var(--forest)}a{color:inherit;text-decoration:none}input,select{color:var(--ink);background:var(--panel);border:1px solid var(--line-strong);border-radius:5px;padding:9px 10px;min-width:0}small{font-size:11px;color:var(--ink-soft)}
    h1{font:600 18px var(--sans);margin:0 0 18px}h2{font:600 14px var(--sans);margin:0}p{color:var(--ink-soft);font-size:13px}summary{cursor:pointer;list-style:none}summary::-webkit-details-marker{display:none}
    .planner-shell{height:100dvh;min-width:880px;min-height:580px;display:flex;flex-direction:column}.site-header{height:48px;flex:none;background:var(--rust);color:var(--cream);display:flex;align-items:center;padding:0 20px;gap:28px}.brand{display:flex;align-items:center;gap:10px}.brand img{width:28px;height:28px;border-radius:6px}.brand span{font:700 17px var(--mono)}.site-header nav{display:flex;gap:24px;font-size:13px;height:100%;align-items:center}.site-header nav span{height:100%;display:flex;align-items:center;border-bottom:3px solid var(--amber)}.theme{margin-left:auto;color:inherit;display:grid;place-items:center;width:32px;height:32px}
    .trip-bar{display:grid;grid-template-columns:calc(var(--side-width) - 18px) minmax(0,1fr) auto;align-items:center;gap:4px 14px;min-height:100px;flex:none;padding:8px 18px;border-bottom:1px solid var(--line);background:var(--panel)}.trip-name{min-width:0}.trip-name strong{display:block;font-size:17px}.planning-tools{grid-column:2/-1;grid-row:2}.edit-actions{grid-column:3;grid-row:1}.planning-tools,.edit-actions{display:flex;align-items:center;gap:5px}.planning-tools button{display:flex;align-items:center;gap:7px;padding:9px 10px;font-weight:550;white-space:nowrap;border-radius:5px;font-size:13px}.planning-tools button.active{background:var(--parchment);color:var(--forest)}.count{background:var(--amber);color:var(--on-amber);border-radius:4px;font:600 11px var(--mono);padding:1px 5px}.edit-actions{margin-left:auto}.icon-button{width:30px;height:30px;display:inline-grid;place-items:center;flex:none}.add-menu{position:relative}.add-menu summary{display:flex;align-items:center;gap:6px;white-space:nowrap;background:var(--ink);color:var(--panel);padding:8px 10px;border-radius:5px;font-size:12px}.add-menu summary.active{background:var(--rust)}.add-menu>div{position:absolute;right:0;top:40px;z-index:20;background:var(--panel);border:1px solid var(--line);box-shadow:0 5px 15px #0002;padding:5px;width:145px;border-radius:5px}.add-menu button{display:flex;gap:9px;padding:9px;width:100%;text-align:left}.add-menu button:hover{background:var(--parchment)}
    main{display:grid;grid-template-columns:var(--side-width) 8px minmax(0,1fr);min-height:0;flex:1}.planner-pane{background:var(--panel);border-right:1px solid var(--line);display:flex;flex-direction:column;min-height:0}.search-dock{padding:0 0 12px}.trip-summary{display:grid;grid-template-columns:1fr .9fr 1.2fr;gap:8px;padding:0 18px 12px;border-bottom:1px solid var(--line);flex:none}.trip-summary>div>strong{display:block;font:600 23px var(--sans);letter-spacing:-.7px;white-space:nowrap}.trip-summary strong small{font:500 12px var(--sans);margin-left:4px;letter-spacing:0}.trip-summary>div>span{font-size:11px;color:var(--ink-soft)}.summary-bottom{grid-column:1/-1;display:flex;justify-content:space-between;align-items:center;margin-top:5px}.summary-bottom button{display:flex;gap:6px;align-items:center;font-size:11px;color:var(--ink-soft)}.pane-scroll{overflow:auto;flex:1;min-height:0}.pane-heading{display:flex;align-items:center;justify-content:space-between;padding:17px 18px 12px}.pane-heading h1{margin:0;font-size:14px}.pane-heading>span{font-size:11px;color:var(--ink-soft)}
    .day-section{border-top:1px solid var(--line)}.day-section:last-child{border-bottom:1px solid var(--line)}.day-heading{display:flex;align-items:center;gap:9px;width:100%;padding:14px 13px 14px 17px;text-align:left}.day-number{width:23px;height:23px;border:1px solid var(--line-strong);border-radius:50%;display:grid;place-items:center;flex:none;font:600 11px var(--mono);color:var(--ink-soft)}.day-section .day-number{background:var(--day-color);border-color:var(--day-color);color:var(--panel)}.expanded .day-heading{background:var(--parchment)}.day-title{flex:1;min-width:0}.day-title strong{font-size:12px;display:block;line-height:1.5}.day-title strong span,.day-title small span{color:var(--ink-faint)}.day-title small{font-size:11px;display:block;margin-top:3px}.day-distance{font:600 19px var(--sans);text-align:right;letter-spacing:-.6px;flex:none}.day-distance small{font:400 10px var(--sans);display:block;letter-spacing:0}.over{color:var(--forest)}.day-content{padding:7px 16px 10px 48px}.stop-line{display:flex;align-items:center;gap:9px;min-height:31px;width:100%;text-align:left;font-size:12px}.stop-line>span:not(.stop-dot){flex:1}.stop-line small{font-size:10px}.stop-dot{width:7px;height:7px;border:1px solid var(--ink-soft);border-radius:50%;margin:0 4px}.stop-line.end{font-weight:600}.day-content .stay-list{margin:5px 0 0;border-left:1px solid var(--line);padding-left:11px}.stay-row{display:flex;gap:10px;align-items:center;text-align:left;width:100%;padding:10px 0;border-bottom:1px solid var(--line)}.day-content .stay-row{padding:8px 0}.stay-row>span{flex:1;min-width:0}.stay-row strong{display:block;font-size:12px;font-weight:550}.stay-row small{display:block;font-size:10px;margin-top:1px}.stay-row>b{font:550 14px var(--sans);text-align:right}.stay-row.selected{color:var(--forest)}.stay-row:last-child{border-bottom:0}.map-pick{display:flex;align-items:center;gap:6px;font-size:11px;margin:9px 0;color:var(--forest)}.day-actions{display:flex;gap:16px;border-top:1px solid var(--line);margin-top:8px;padding-top:9px}.day-actions button{display:flex;align-items:center;gap:5px;font-size:10px;color:var(--ink-soft)}.rest-row{display:flex;align-items:center;gap:10px;padding:12px 17px;border-top:1px solid var(--line);background:var(--parchment)}.rest-row>div{flex:1}.rest-row strong,.rest-row small{display:block;font-size:12px}.rest-row small{font-size:11px}.rest{border-style:dashed}
    .detail-panel{padding:18px}.detail-panel .pane-heading{padding:8px 0 12px}.back{display:flex;align-items:center;gap:6px;color:var(--ink-soft);font-size:12px;margin-bottom:20px}.budget-form{display:flex;flex-direction:column;gap:8px}.budget-form label{font-size:12px}.budget-form h2{margin-top:18px}.budget-form select{margin-bottom:6px}.form-help,.fixture-note{font-size:11px;color:var(--ink-soft);line-height:1.5}.primary{background:var(--amber);color:var(--on-amber);border:1px solid color-mix(in srgb,var(--amber) 80%,var(--ink));border-radius:5px;display:flex;align-items:center;justify-content:space-between;gap:12px;padding:9px 11px;font-weight:600;font-size:12px}.primary:hover{filter:brightness(.97);color:var(--on-amber)}.secondary{border:1px solid var(--line);border-radius:5px;padding:7px 10px;font-size:12px}.route-option{display:flex;gap:11px;width:100%;align-items:center;text-align:left;border-bottom:1px solid var(--line);padding:17px 0}.route-option>span:nth-child(2){flex:1}.route-option strong{display:block;font-size:13px}.route-option small{display:block;font-size:11px}.route-option b{text-align:right;font-size:18px;font-weight:550}.radio-mark{width:16px;height:16px;border:1px solid var(--line-strong);border-radius:50%;display:grid;place-items:center;font-size:10px}.chosen .radio-mark{border-color:var(--forest);color:var(--forest)}.check-row{display:flex;gap:12px;padding:16px 0;border-bottom:1px solid var(--line);color:var(--forest)}.check-row>div{flex:1}.check-row h2{font-size:13px;color:var(--ink)}.check-row p{font-size:12px;margin:5px 0 10px}.check-row button{font-size:12px;text-decoration:underline;text-underline-offset:3px}.inline-actions{display:flex;gap:20px}.inline-warning{display:flex;align-items:center;gap:6px;color:var(--forest);font-size:11px;margin:8px 0}.inline-warning span{flex:1}.inline-warning button{text-decoration:underline;white-space:nowrap}.all-clear{display:flex;gap:8px;align-items:center;font-size:12px}.muted{color:var(--ink-soft)}
    .geography{display:flex;flex-direction:column;min-width:0;min-height:0}.map-area{position:relative;min-height:170px;flex:1}.map-controls{position:absolute;right:16px;top:16px;display:flex;flex-direction:column;gap:9px;z-index:3}.control-group,.layer-menu{background:var(--panel);border:1px solid var(--line);border-radius:6px;box-shadow:0 2px 8px #0002}.control-group button,.layer-menu summary{display:grid;place-items:center;width:34px;height:34px}.control-group button+button{border-top:1px solid var(--line)}.control-group .chosen{color:var(--forest)}.layer-menu{position:relative}.layer-menu>div{position:absolute;right:43px;top:0;background:var(--panel);border:1px solid var(--line);box-shadow:0 2px 8px #0002;border-radius:6px;padding:13px;min-width:155px;font-size:12px}.layer-menu strong{display:block;margin-bottom:9px}.layer-menu label{display:flex;align-items:center;gap:7px;padding:4px 0}.layer-menu input{accent-color:var(--forest)}.status-line{height:27px;display:flex;align-items:center;gap:10px;padding:0 16px;border-top:1px solid var(--line);background:var(--panel);font-size:10px;color:var(--ink-soft);flex:none}.status-line button{text-decoration:underline;color:var(--forest)}.status-meta{margin-left:auto}
    .point-popup{width:276px;padding:15px;position:relative;color:var(--ink);font:13px var(--sans)}.popup-close{position:absolute;right:8px;top:8px;display:grid;place-items:center;width:23px;height:23px;color:var(--ink-soft)}.point-popup h2{font-size:14px;margin:0 20px 12px 0}.popup-title{display:flex;align-items:center;padding-right:17px;margin-bottom:9px}.popup-title h2{margin:0;flex:1}.point-popup input{width:100%;font-size:12px;margin-bottom:10px}.point-popup .primary{width:100%;margin-top:8px}.point-popup .secondary{width:100%;margin-top:7px}.popup-distance{display:flex;justify-content:space-between;font-size:12px;margin-bottom:9px}.popup-distance>span{color:var(--ink-soft)}.point-types{display:flex;border:1px solid var(--line);border-radius:5px;overflow:hidden}.point-types button{flex:1;display:flex;align-items:center;flex-direction:column;gap:5px;padding:9px 2px;font-size:9px;white-space:nowrap}.point-types button+button{border-left:1px solid var(--line)}.point-types .chosen,.reach-options .chosen{background:var(--parchment);color:var(--forest)}.reach-options{display:flex;gap:5px;margin-top:8px}.reach-options button{flex:1;padding:7px 4px;border:1px solid var(--line);border-radius:4px;font-size:11px}.remove{font-size:11px;color:var(--ink-soft);margin-top:13px}
    .add-marker{display:flex;align-items:center;gap:6px;margin-top:10px;font-size:11px;color:var(--ink-soft)}.point-popup .stay-list{margin:-4px 0 4px}.popup-hint{font-size:11px;margin:4px 0 0}
    .mode-button{display:flex;align-items:center;gap:5px;font-size:11px;color:var(--ink-soft);margin-top:3px}.length-track{display:block;height:3px;background:var(--line);margin-top:7px;width:100%;border-radius:2px}.length-track>span{display:block;height:100%;background:var(--day-color);border-radius:2px}.route-add{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-top:20px;width:100%}.sleep-target{display:block;font-size:11px;margin:8px 0;color:var(--ink-soft)}.sleep-target select{display:block;width:100%;font-size:12px;margin-top:5px}.day-actions button{font-size:12px}
    .ride-controls{display:flex;gap:12px;align-items:center;grid-column:2;grid-row:1}.ride-controls label{display:flex;align-items:center;gap:7px;font-size:12px;color:var(--ink-soft)}.ride-controls select{padding:5px 7px;font-size:13px;font-weight:600;max-width:170px}.route-stop{display:flex;align-items:center;gap:8px;border-bottom:1px solid var(--line);padding:10px 0}.stop-detail{display:flex;align-items:center;gap:10px;flex:1;text-align:left}.stop-detail strong{display:block;font-size:13px}.stop-detail small{display:block;margin-top:4px}.reorder{display:flex;flex-direction:column}.reorder button{width:26px;height:25px;display:grid;place-items:center}.between-days{display:flex;align-items:center;gap:6px;width:100%;padding:7px 18px 7px 49px;font-size:11px;color:var(--ink-soft);border-top:1px solid var(--line)}.between-days:hover{background:var(--parchment)}.rest-name{display:flex;align-items:center;gap:8px;text-align:left}.rest-row input{width:100%;font-size:12px;padding:5px}.rest-row>div{min-width:0}
    @media(max-width:1150px){.trip-bar{gap:10px}.trip-name{width:auto}.planning-tools button{padding:8px;font-size:12px;gap:5px}.planning-tools button:first-child :global(svg){display:none}.brand span{font-size:15px}.add-menu summary{font-size:11px}.edit-actions{gap:0}main{grid-template-columns:var(--side-width) 8px minmax(0,1fr)}}
</style>
