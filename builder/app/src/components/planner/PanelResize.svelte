<script lang="ts">
    let { value, min, max, axis, label, onResize }: { value:number; min:number; max:number; axis:'x'|'y'; label:string; onResize:(value:number)=>void } = $props();
    let start: {position:number;value:number}|null = null;
    function update(value:number){onResize(Math.round(Math.max(min,Math.min(max,value))));}
    function down(event:PointerEvent){event.preventDefault();event.currentTarget && (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);start={position:axis==='x'?event.clientX:event.clientY,value};}
    function move(event:PointerEvent){if(start)update(start.value+((axis==='x'?event.clientX:event.clientY)-start.position)*(axis==='x'?1:-1));}
    function key(event:KeyboardEvent){const steps:Record<string,number>={ArrowLeft:-16,ArrowRight:16,ArrowUp:16,ArrowDown:-16};if(event.key in steps){event.preventDefault();update(value+steps[event.key]);}else if(event.key==='Home'){event.preventDefault();update(min);}else if(event.key==='End'){event.preventDefault();update(max);}}
</script>
<!-- A focusable separator implements the ARIA window splitter pattern. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div role="separator" tabindex="0" aria-label={label} aria-orientation={axis==='x'?'vertical':'horizontal'} aria-valuenow={value} aria-valuemin={min} aria-valuemax={max} class:vertical={axis==='x'} onpointerdown={down} onpointermove={move} onpointerup={()=>start=null} onpointercancel={()=>start=null} onkeydown={key}><span></span></div>
<style>
    div{height:8px;flex:none;display:flex;align-items:center;justify-content:center;cursor:row-resize;touch-action:none;background:var(--panel);border-top:1px solid var(--line)}
    span{height:2px;width:36px;background:var(--line-strong);border-radius:2px}.vertical{width:8px;height:100%;cursor:col-resize;border-top:0;border-left:1px solid var(--line)}.vertical span{width:2px;height:36px}
    div:hover span,div:focus-visible span{background:var(--forest)}div:focus-visible{outline:2px solid var(--forest);outline-offset:-2px}
</style>
