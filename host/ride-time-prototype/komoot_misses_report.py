"""Private case reports and motion audit for the remaining Komoot ETA misses."""

import argparse
import json
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np

from komoot_data import read_gpx
from komoot_misses import OUTPUT, PREPARED, SOURCE
from komoot_motion import stationary_candidates
from komoot_report import Report
from long_data import digest

NAMES=dict(original='Original live',aggregate='Since departure',recent30='Last 30 min',early_grade='First-hour grade factors')
COLORS=dict(original='#888888',aggregate='#365d3c',recent30='#be773f',early_grade='#7364a7')


def verify(output):
    run=json.loads((output/'run.json').read_text())
    if any(digest(output/name)!=h for name,h in run['hashes'].items()):raise ValueError('Diagnostic results changed')
    plan=json.loads((output/'plan.json').read_text())
    root=Path(__file__).resolve().parent
    if any(digest(root/name)!=h for name,h in plan['code_hashes'].items()):raise ValueError('Diagnostic sources changed')


def audit(source,prepared,output):
    verify(output)
    root=Path(__file__).resolve().parent
    plan=dict(method='Disjoint approximately 120 wall-second windows, no unknown intervals or explicit pauses; bounding-box diagonal <=30m and >=1 accepted moving minute. Candidate drift only; preserve all observations and outcomes.',
        hashes={str((root/n).resolve()):digest(root/n) for n in ('komoot_motion.py','komoot_misses_report.py','tests/test_komoot_motion.py')},
        run_sha256=digest(output/'run.json'),source_manifest_sha256=digest(source/'manifest.json'))
    with (output/'motion-plan.json').open('x') as f:json.dump(plan,f,indent=2)
    files={r['id']:r for r in json.loads((source/'manifest.json').read_text())['files']}
    rows={}
    for c in json.loads((output/'cases.json').read_text()):
        file=files[c['ride']];path=source/file['path']
        if digest(path)!=file['sha256']:raise ValueError('GPX changed')
        points,_=read_gpx(path)
        with np.load(prepared/(c['ride']+'.npz')) as f:data={k:f[k] for k in f.files}
        rows[c['ride']]=stationary_candidates(points,data,c['at_minutes'])
    (output/'motion.json').write_text(json.dumps(rows,indent=2)+'\n')
    (output/'motion-run.json').write_text(json.dumps(dict(hashes={n:digest(output/n) for n in ('motion-plan.json','motion.json')}),indent=2)+'\n')
    print('Motion audit complete')


def figure(fig,output,name,r,caption):
    fig.savefig(output/(name+'.svg'));fig.savefig(output/(name+'.png'),dpi=125);plt.close(fig)
    r.figure(output/(name+'.svg'),caption)


def geometry(source,output,cases,motion,r):
    worst=max(cases,key=lambda c:abs(c['signed_percent']))
    spans=[s for s in motion[worst['ride']] if s['early_accepted_min']>0]
    groups=[]
    for s in spans:
        if groups and abs(groups[-1][-1]['end_wall_min']-s['start_wall_min'])<1e-7:groups[-1].append(s)
        else:groups.append([s])
    groups=sorted(groups,key=lambda x:x[-1]['end_wall_min']-x[0]['start_wall_min'],reverse=True)[:2]
    if not groups:return
    a,_=read_gpx(source/'downloads'/(worst['ride']+'.gpx'))
    wall=(a[:,0]-a[0,0])/60
    fig,axes=plt.subplots(1,len(groups),figsize=(10,4.5),constrained_layout=True,squeeze=False)
    facts=[]
    for ax,group in zip(axes[0],groups):
        lo=group[0]['start_wall_min'];hi=group[-1]['end_wall_min'];v=a[(wall>=lo-1e-7)&(wall<=hi+1e-7)]
        lat=np.radians(v[:,1]);lon=np.radians(v[:,2]);xy=np.column_stack([(lon-lon[0])*6371000*np.cos(lat.mean()),(lat-lat[0])*6371000])
        elapsed=(v[:,0]-v[0,0])/60
        ax.plot(xy[:,0],xy[:,1],color='#999',linewidth=.7);sc=ax.scatter(xy[:,0],xy[:,1],c=elapsed,s=12,cmap='viridis')
        ax.set(title=f'{lo:.1f}–{hi:.1f} wall minutes after departure',xlabel='East from first point (m)',ylabel='North from first point (m)',aspect='equal');ax.grid(alpha=.2)
        fig.colorbar(sc,ax=ax,label='Minutes within this span')
        facts.append(dict(start_wall_min=lo,end_wall_min=hi,duration_min=hi-lo,box_diagonal_m=float(np.linalg.norm(np.ptp(xy,axis=0))),net_m=float(np.linalg.norm(xy[-1])),points=len(v)))
    figure(fig,output,'stationary_geometry',r,'Two longest contiguous groups of drift candidates intersecting the first observed hour of the largest original miss. Coordinates are relative metres; colour is time. These traces support a stationary-drift interpretation, but the detector is not validated.')
    (output/'geometry-inspection.json').write_text(json.dumps(dict(ride=worst['ride'],spans=facts),indent=2)+'\n')


def report(output,source=SOURCE):
    verify(output)
    motion_run=json.loads((output/'motion-run.json').read_text())
    if any(digest(output/n)!=h for n,h in motion_run['hashes'].items()):raise ValueError('Motion audit changed')
    cases=json.loads((output/'cases.json').read_text())
    rows=json.loads((output/'predictions.json').read_text())
    summary=json.loads((output/'summary.json').read_text())
    motion=json.loads((output/'motion.json').read_text())
    selected=sorted([c for c in cases if c['detailed']],key=lambda c:abs(c['signed_percent']),reverse=True)
    r=Report();r.heading('Why do the remaining ride-time estimates miss?',1)
    r.paragraph('Private exploratory case review. Same 66 previously inspected long rides, evaluated after one hour of accepted moving time. Predictions concern moving sections only; breaks and unknown recording intervals are excluded. No shared defaults or production estimator changed.')
    if (output/'interpretation.json').exists():
        interpretation=json.loads((output/'interpretation.json').read_text())
        r.heading('Findings')
        for s in interpretation['findings']:r.paragraph(s)
        r.heading('Decision');r.paragraph(interpretation['decision'])
    r.heading('1. Does any fixed diagnostic reach dependable single-digit error?')
    r.paragraph('All methods use only observations available at the same one-hour checkpoint. The last-30-minute probe is an aggregate time/default-time ratio. The grade probe uses separate first-hour ratios in eight fixed gradient bands with at least 400 m of early support; unsupported grades fall back to the scalar ratio. These simple probes have no new outlier bounds or calibrated uncertainty ranges.')
    table=[]
    for s in summary:
        if s['scope']=='accepted' and s['horizon']=='remaining':
            table.append([NAMES[s['mode']],s['n'],f"{s['mape']:.1f}%",f"{s['median_ape']:.1f}%",f"{s['p90_ape']:.1f}%",f"{s['mae_minutes']:.1f}",f"{s['within5']}/{s['n']}",f"{s['within10']}/{s['n']}"])
    r.table(['Method','Rides','Mean APE','Median APE','90th percentile','Mean abs. min','Within 5%','Within 10%'],table)
    fig,ax=plt.subplots(figsize=(9,4),constrained_layout=True)
    for mode in NAMES:
        a=sorted(100*abs(x['predicted_minutes']/x['actual_minutes']-1) for x in rows if x['mode']==mode and x['horizon']=='remaining')
        ax.step(a,np.arange(1,len(a)+1)/len(a)*100,where='post',label=NAMES[mode],color=COLORS[mode])
    for x in (5,10):ax.axvline(x,color='#aaa',linestyle=':')
    ax.set(xlabel='Absolute remaining-time error (%)',ylabel='Rides at or below this error (%)',xlim=(0,90),ylim=(0,100));ax.grid(alpha=.2);ax.legend(fontsize=9)
    figure(fig,output,'error_distribution',r,'All targets remain included. A single-digit median describes only the middle ride; it does not establish reliability across rides.')
    r.heading('2. Shorter horizons: percentage error versus minutes')
    r.paragraph('Each target begins at the same one-hour checkpoint. Include it only if at least the requested accepted distance remains; prorate the last recorded interval. Methods share targets within each horizon. The 20 km cohort is smaller, so cross-horizon comparisons are descriptive.')
    table=[]
    for s in summary:
        if s['mode']=='aggregate' and s['n']:
            table.append([s['horizon'],s['scope'],s['n'],f"{s['mape']:.1f}%",f"{s['p90_ape']:.1f}%",f"{s['mae_minutes']:.1f}",f"{s['within10']}/{s['n']}"])
    r.table(['Horizon km','Sections','Rides','Mean APE','90th percentile','Mean abs. min','Within 10%'],table)
    fig,axes=plt.subplots(1,2,figsize=(11,4),constrained_layout=True)
    for mode in NAMES:
        a=[next(s for s in summary if s['mode']==mode and s['scope']=='accepted' and s['horizon']==h) for h in ('5','10','20','remaining')]
        for ax,key in zip(axes,('mape','mae_minutes')):ax.plot(range(4),[s[key] for s in a],'o-',label=NAMES[mode],color=COLORS[mode])
    for ax,label in zip(axes,('Mean absolute percentage error (%)','Mean absolute error (minutes)')):
        ax.set(xticks=range(4),xticklabels=['5 km','10 km','20 km','Remaining'],ylabel=label);ax.grid(alpha=.2)
    axes[0].legend(fontsize=8)
    figure(fig,output,'horizons',r,'Short targets can have larger percentage errors but smaller errors in minutes. Accepted targets can cross omitted gaps; contiguous targets contain no unknown intervals, but still use the same imperfect moving-time proxy.')
    r.heading('3. Grade mismatch versus changes within a grade band')
    r.paragraph('For each ride, split the signed aggregate error into two terms: (aggregate prediction − first-hour-grade prediction) + (first-hour-grade prediction − actual remaining time). The sum is exact. The first term measures sensitivity to different grade factors and route mix. The second includes pace changes within supported bands and unknown behaviour in unsupported bands. Neither term proves a physical cause.')
    fig,axes=plt.subplots(1,2,figsize=(11,4.5),constrained_layout=True)
    ordered=sorted(cases,key=lambda c:c['signed_percent'])
    x=np.arange(len(ordered));mix=np.array([sum(t['mix_component'] for t in c['terrain'])/c['actual_minutes']*100 for c in ordered]);within=np.array([sum(t['within_component'] for t in c['terrain'])/c['actual_minutes']*100 for c in ordered])
    axes[0].plot(x,mix,label='Grade-factor/mix term',color=COLORS['early_grade']);axes[0].plot(x,within,label='Within-band / unsupported term',color=COLORS['recent30']);axes[0].plot(x,mix+within,label='Total error',color=COLORS['aggregate'])
    axes[0].set(xlabel='Rides ordered by signed total error',ylabel='Signed error (% of remaining time)');axes[0].legend(fontsize=8)
    axes[1].scatter([c['unsupported_future_cost_fraction']*100 for c in cases],[abs(c['signed_percent']) for c in cases],color=COLORS['aggregate'])
    axes[1].set(xlabel='Remaining default time in unsupported grades (%)',ylabel='Absolute aggregate error (%)')
    for ax in axes:ax.axhline(0,color='#777',linestyle=':');ax.grid(alpha=.2)
    figure(fig,output,'decomposition',r,'Opposite signs can cancel. Do not turn these signed terms into percentages of explained variance or interpret the residual as an irreducible accuracy floor.')
    r.heading('4. Possible stationary GPS drift')
    flagged=[c for c in cases if motion[c['ride']]]
    total=sum(sum(s['early_accepted_min']+s['later_accepted_min'] for s in motion[c['ride']]) for c in cases)
    r.paragraph(f"A separate geometry audit flags disjoint approximately two-minute spans confined to a 30 m bounding-box diagonal, with at least one accepted moving minute and no unknown interval or explicit pause. It flags {total:.1f} accepted minutes across {len(flagged)} rides. These are inspection candidates, not verified stops. Slow pushing can also remain within a small area. All results above retain the original observations and targets; the separate sensitivity below changes the motion proxy.")
    geometry(source,output,cases,motion,r)
    if (output/'sensitivity-run.json').exists():
        manifest=json.loads((output/'sensitivity-run.json').read_text())
        if any(digest(output/n)!=h for n,h in manifest['hashes'].items()):raise ValueError('Sensitivity results changed')
        sensitivity=json.loads((output/'sensitivity.json').read_text())
        r.heading('Sensitivity to the motion definition',3)
        r.paragraph('Keep the original forecast instants and route costs. Exclude only candidate spans completed by the checkpoint from past aggregate time/default cost. Separately subtract candidate spans from the future outcome. A span crossing the checkpoint cannot revise the past. The table crosses both forecasts with both outcome definitions; this exposes how the measurement target affects the result. It is not validated moving-time accuracy, and the checkpoint is no longer one full cleaned moving hour.')
        r.table(['Past observations','Outcome','Mean APE','Median APE','90th percentile','Mean abs. min','Within 10%'],[[s['mode'],s['target'],f"{s['mape']:.1f}%",f"{s['median_ape']:.1f}%",f"{s['p90_ape']:.1f}%",f"{s['mae_minutes']:.1f}",f"{s['within10']}/{s['n']}"] for s in sensitivity['summary']])
        worst=max(cases,key=lambda c:abs(c['signed_percent']));detail=next(d for d in sensitivity['details'] if d['ride']==worst['ride'])
        r.paragraph(f"Largest original miss ({worst['date'][:10]}): {detail['early_removed_min']:.1f} early minutes were in completed candidates. At the same forecast instant, {detail['past_accepted_min']:.1f} accepted minutes remain after this exclusion. Its signed error changes from {worst['signed_percent']:+.1f}% to {detail['clean_signed_percent']:+.1f}% against the alternate outcome. This is strong evidence that the original miss cannot be attributed entirely to changing rider pace.")
        lookup={c['ride']:c for c in cases}
        remaining=sorted(sensitivity['details'],key=lambda d:abs(d['clean_signed_percent']),reverse=True)[:5]
        r.table(['Largest alternate-proxy misses','Original error %','Alternate error %','Early excluded min','Later excluded min'],[[lookup[d['ride']]['date'][:10],f"{lookup[d['ride']]['signed_percent']:+.1f}",f"{d['clean_signed_percent']:+.1f}",f"{d['early_removed_min']:.1f}",f"{d['later_removed_min']:.1f}"] for d in remaining])
    r.table(['Date','Candidate early min','Candidate later min','Aggregate error min'],[[c['date'][:10],f"{sum(s['early_accepted_min'] for s in motion[c['ride']]):.1f}",f"{sum(s['later_accepted_min'] for s in motion[c['ride']]):.1f}",f"{c['error_minutes']:+.1f}"] for c in sorted(flagged,key=lambda c:sum(s['early_accepted_min']+s['later_accepted_min'] for s in motion[c['ride']]),reverse=True)])
    r.heading('5. Largest misses, including errors in both directions')
    r.paragraph('Select the union of the ten largest absolute percentage errors and five largest absolute minute errors of the aggregate forecast. Positive error means arrival was predicted too late. Time factors compare accepted time with the fixed default gradient/bike curve; a smaller factor means faster riding after this adjustment.')
    r.table(['Date','Ride km','Moving h','Error min','Error %','Early factor','Later factor'],[[c['date'][:10],f"{c['km']:.1f}",f"{c['moving_minutes']/60:.1f}",f"{c['error_minutes']:+.1f}",f"{c['signed_percent']:+.1f}%",f"{c['early_factor']:.2f}",f"{c['later_factor']:.2f}"] for c in selected])
    for number,c in enumerate(selected,1):
        r.heading(f"Case {number}: {c['date'][:10]} · {c['km']:.1f} km",3)
        r.paragraph(f"Recorded ride {c['ride']}; {c['bike']} bike category. At {c['at_minutes']:.1f} observed moving minutes, prediction {c['predicted_minutes']:.1f} min versus actual remaining {c['actual_minutes']:.1f} min. {100*c['unsupported_future_cost_fraction']:.1f}% of future default time is in grade bands with less than 400 m of early evidence. {c['unknown_minutes']:.1f} unknown recording minutes are excluded from the entire ride. {100*c['slow_time_fraction']:.1f}% of accepted time has interval speed below 4 km/h; this is not a stop classification.")
        fig,axes=plt.subplots(1,2,figsize=(12,4.2),constrained_layout=True)
        trace=c['trace'];axes[0].plot([t['minute']/60 for t in trace],[t['factor'] for t in trace],'o-',markersize=3,color=COLORS['aggregate'])
        axes[0].axvline(c['at_minutes']/60,color='#555',linestyle=':')
        axes[0].axhline(c['early_factor'],label='First-hour aggregate',color=COLORS['early_grade'],linestyle='--');axes[0].axhline(c['later_factor'],label='Remaining aggregate',color=COLORS['recent30'],linestyle='--')
        axes[0].set(xlabel='Accepted moving hours',ylabel='Time / fixed default time (10 min spans)');axes[0].legend(fontsize=8)
        terrain=c['terrain'];k=np.arange(len(terrain));early=[t['early_factor'] if t['supported'] else np.nan for t in terrain];late=[t['later_factor'] if t['future_km']>=.4 else np.nan for t in terrain]
        axes[1].plot(k,early,'o-',label='First hour',color=COLORS['early_grade']);axes[1].plot(k,late,'o-',label='Later',color=COLORS['recent30'])
        axes[1].set(xticks=k,xticklabels=['<−6','−6:−3','−3:−1','−1:1','1:3','3:6','6:10','>10'],xlabel='Gradient band (%)',ylabel='Time / default time within grade');axes[1].tick_params(axis='x',labelsize=8);axes[1].legend(fontsize=8)
        for ax in axes:ax.grid(alpha=.2)
        figure(fig,output,f'case_{number:02d}',r,'Left: observed pace changes over the ride; unknown time is omitted. Right: early/later factors within the same grade bands, showing only periods with at least 400 m support. Similar gradients do not establish identical surfaces, wind, or conditions.')
        r.table(['Grade','Early km','Later km','Scalar error min','Grade/mix term','Within / unsupported term'],[[t['grade'],f"{t['early_km']:.1f}",f"{t['future_km']:.1f}",f"{t['scalar_error']:+.1f}",f"{t['mix_component']:+.1f}",f"{t['within_component']:+.1f}"] for t in terrain if t['future_km']>0])
    r.heading('Limits and reproduction')
    r.paragraph('This is a single rider, with no verified per-ride companion, luggage, wind, surface, or motion-state labels. Recorded future geometry substitutes for a known planned route. The same data have already influenced our hypotheses; none of these comparisons validates a selected replacement model. Whole-route ranges and nRF54LM20 resource use are outside this diagnostic.')
    r.paragraph('Run komoot_misses.py once, komoot_misses_report.py audit once, optionally komoot_motion_sensitivity.py once, then komoot_misses_report.py report. Plans refuse overwrites. Results, source GPX, case identifiers, plots, and optional interpretation.json remain local. Report generation verifies computation hashes and does not repeat predictions.')
    r.save(output)
    files=sorted(output.glob('*.svg'))+[output/'motion-run.json']
    files += [output/n for n in ('sensitivity-run.json','geometry-inspection.json') if (output/n).exists()]
    if (output/'interpretation.json').exists():files.append(output/'interpretation.json')
    (output/'report-inputs.json').write_text(json.dumps(dict(report_source_sha256=digest(Path(__file__)),run_sha256=digest(output/'run.json'),files={p.name:digest(p) for p in files}),indent=2)+'\n')
    print(output/'report.html')


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('action',choices=('audit','report'))
    for name,default in [('source',SOURCE),('prepared',PREPARED),('output',OUTPUT)]:p.add_argument('--'+name,type=Path,default=default)
    a=p.parse_args()
    if a.action=='audit':audit(a.source,a.prepared,a.output)
    else:report(a.output,a.source)
