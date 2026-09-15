"""Private figures explaining speed variability and exploratory ETA errors."""

import argparse
from datetime import datetime
import json
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np

from komoot_data import SOURCE
from komoot_explore import GRADE_EDGES, OUTPUT
from komoot_report import Report
from long_data import digest

COLORS = {'short':'#365d3c','long':'#cf744e','mtb':'#695ca2'}


def save(fig, output, name, report, caption):
    fig.savefig(output/(name+'.svg'))
    fig.savefig(output/(name+'.png'),dpi=135)
    plt.close(fig)
    report.figure(output/(name+'.svg'),caption)


def phase_stats(phases, kind, labels, ids=None):
    result=[]
    for label in labels:
        a=np.array([p['ratio'] for p in phases if p['kind']==kind and p['phase']==label
                    and (ids is None or p['ride'] in ids)])
        result.append((len(a),np.quantile(a,[.1,.5,.9]) if len(a) else np.full(3,np.nan)))
    return result


def report(source, output):
    run=json.loads((output/'run.json').read_text())
    if any(digest(output/name)!=expected for name,expected in run['hashes'].items()):
        raise ValueError('Exploration outputs changed')
    plan=json.loads((output/'plan.json').read_text())
    root=Path(__file__).resolve().parent
    if any(digest(root/name)!=expected for name,expected in plan['code_hashes'].items()):
        raise ValueError('Exploration source changed')
    summary=json.loads((output/'summary.json').read_text())
    rides=json.loads((output/'rides.json').read_text())
    phases=json.loads((output/'phases.json').read_text())
    probes=json.loads((output/'probes.json').read_text())
    source_rides=json.loads((source/'manifest.json').read_text())['rides']
    with np.load(output/'windows.npz') as data:w=data['windows']
    r=Report();r.heading('Where does your riding-time uncertainty come from?',1)
    r.paragraph('Private exploratory analysis of your complete exported cycling history. These are already-inspected data, not a fresh validation set. All source summaries are included in the inventory; overlapping recordings are excluded from the section analysis.')
    if (output/'interpretation.json').exists():
        interpretation=json.loads((output/'interpretation.json').read_text())
        r.heading('Main findings')
        for finding in interpretation['findings']:r.paragraph(finding)
        r.heading('Recommended next step')
        r.paragraph(interpretation['next_step'])
    r.heading('Questions and reading guide')
    r.paragraph('The plots distinguish ride-to-ride pace differences, variation within a ride, and effects introduced by the replay. Individual rides are not labelled, so the plots cannot assign an effect to riding together, luggage, fitness, wind, surface, or fatigue.')
    if (output/'context.json').exists():r.paragraph(json.loads((output/'context.json').read_text())['rider_context'])
    r.paragraph('Shaded bands show the middle 80% of observed ride values (10th–90th percentiles). They are distributions, not confidence intervals or calibrated ETA ranges. Window speeds use approximately 200 m of uninterrupted accepted movement to reduce point-level GPS noise. Pushing has no minimum-speed exclusion.')
    r.table(['Input','Count'],[['Source cycling recordings',len(source_rides)],['Non-overlapping prepared recordings',len(rides)],
        ['200 m windows',len(w)],['Accepted distance represented by windows',f"{100*summary['window_km']/summary['accepted_km']:.1f}%"],
        ['Frozen long evaluation targets used for probes',summary['probes']['reset']['60']['n']]])
    r.heading('1. Ride length and average speed')
    fig,axes=plt.subplots(1,2,figsize=(11,4.5),constrained_layout=True)
    for sport,color,label in [('touringbicycle',COLORS['short'],'Other cycling'),('racebike','#3b8fa3','Road label'),('mtb',COLORS['mtb'],'MTB label')]:
        a=[x for x in source_rides if x['sport']==sport and x['time_in_motion']>0 and x['distance']>0]
        speed=[x['distance']/x['time_in_motion']*3.6 for x in a]
        axes[0].scatter([x['distance']/1000 for x in a],speed,s=17,alpha=.55,color=color,label=label)
        axes[1].scatter([x['time_in_motion']/3600 for x in a],speed,s=17,alpha=.55,color=color,label=label)
    axes[0].set(xscale='log',xlabel='Recorded distance (km, log scale)',ylabel='Source average moving speed (km/h)')
    axes[1].set(xlabel='Source moving hours',ylabel='Source average moving speed (km/h)')
    for ax in axes:ax.grid(alpha=.2);ax.legend(fontsize=8)
    save(fig,output,'ride_inventory',r,'All source summaries. Distance is shown alongside duration because duration and average speed are mathematically coupled: slower riding itself makes a ride longer.')
    r.heading('2. Speed at the same gradient')
    speed=np.array([x['grade_speeds'] for x in rides])
    groups={'short':np.array([x['bike']=='other' and x['moving_minutes']<60 for x in rides]),
            'long':np.array([x['bike']=='other' and x['moving_minutes']>=120 for x in rides]),
            'mtb':np.array([x['bike']=='mtb' for x in rides])}
    labels={'short':'Other bike, under 1 h','long':'Other bike, 2+ h','mtb':'MTB, all durations'}
    centers=(GRADE_EDGES[:-1]+GRADE_EDGES[1:])*50
    fig,(ax,support)=plt.subplots(2,1,figsize=(9,6.5),sharex=True,gridspec_kw={'height_ratios':[4,1]},constrained_layout=True)
    table=[]
    for group,mask in groups.items():
        n=np.sum(np.isfinite(speed[mask]),axis=0)
        quantiles=np.array([np.quantile(a[np.isfinite(a)],[.1,.5,.9]) if np.isfinite(a).sum()>=5 else [np.nan]*3 for a in speed[mask].T]).T
        ax.plot(centers,quantiles[1],marker='o',color=COLORS[group],label=labels[group])
        ax.fill_between(centers,quantiles[0],quantiles[2],alpha=.14,color=COLORS[group])
        support.plot(centers,n,marker='.',color=COLORS[group])
        for bin_index,bin_label in [(4,'-1% to +1%'),(6,'+3% to +6%')]:
            a=speed[mask,bin_index];a=a[np.isfinite(a)]
            if len(a):table.append([labels[group],bin_label,len(a),*[f'{x:.1f}' for x in np.quantile(a,[.1,.5,.9])]])
    ax.set(ylabel='Per-ride aggregate speed in grade bin (km/h)');ax.legend(fontsize=9);ax.grid(alpha=.2)
    support.set(xlabel='Gradient (%)',ylabel='Rides');support.grid(alpha=.2)
    save(fig,output,'speed_gradient',r,'Equal ride weighting: each ride contributes one speed per bin with at least 500 m support. Bands require five rides. MTB is separated because source bike categories and route conditions differ.')
    r.table(['Group','Gradient','Rides','10th percentile km/h','Median km/h','90th percentile km/h'],table)
    quality=np.array([x['proxy_eligible'] for x in rides])
    table=[]
    for group in ('short','long'):
        a=speed[groups[group]&quality,4];a=a[np.isfinite(a)]
        if len(a):table.append([labels[group],len(a),f'{np.median(a):.1f}'])
    r.paragraph('Quality sensitivity: the near-flat comparison below uses only the earlier high-coverage proxy subset. It checks whether incomplete recordings explain the short/long difference.')
    r.table(['High-coverage group','Rides with flat support','Median flat speed km/h'],table)
    r.paragraph('The gradient is the distance-weighted mean of the existing causal 200 m profile within each window. Grade smoothing, GPS errors, corners, short stops, surface, and wind can all contribute to the spread; it is not a measurement of unavoidable biological variability.')
    r.heading('3. Speed distributions within selected terrain')
    fig,axes=plt.subplots(1,3,figsize=(12,4),constrained_layout=True)
    for ax,(name,low,high) in zip(axes,[('Descents',-.06,-.03),('Near-flat',-.01,.01),('Climbs',.03,.06)]):
        for group in ('short','long'):
            allowed=np.flatnonzero(groups[group]);mask=np.isin(w[:,0],allowed)&(w[:,3]>=low)&(w[:,3]<high)
            values=w[mask];weights=np.zeros(len(values))
            for id in np.unique(values[:,0]):
                ix=values[:,0]==id;weights[ix]=values[ix,1]/values[ix,1].sum()
            if len(values):ax.hist(60*values[:,1]/values[:,2],bins=np.arange(0,82,2),weights=weights,density=True,histtype='step',linewidth=2,color=COLORS[group],label=labels[group])
        ax.set(title=name,xlabel='200 m window speed (km/h)',ylabel='Relative density');ax.grid(alpha=.2)
    axes[0].legend(fontsize=8)
    save(fig,output,'terrain_distributions',r,'Window distributions give each contributing ride equal total weight within the terrain band. Short/long labels are not verified solo/partner labels; apparent peaks do not establish distinct rider modes.')
    r.heading('4. Whole-ride pace after accounting for the default gradient curve')
    fig,axes=plt.subplots(1,2,figsize=(11,4.5),constrained_layout=True)
    for group,mask in groups.items():
        a=[x for i,x in enumerate(rides) if mask[i] and x['time_factor'] is not None]
        axes[0].scatter([x['distance_km'] for x in a],[x['time_factor'] for x in a],s=17,alpha=.6,color=COLORS[group],label=labels[group])
        axes[1].scatter([datetime.fromisoformat(x['date'].replace('Z','+00:00')) for x in a],[x['time_factor'] for x in a],s=17,alpha=.6,color=COLORS[group])
    axes[0].set(xscale='log',xlabel='Observed distance (km, log scale)',ylabel='Observed time / default estimated time')
    axes[1].set(xlabel='Ride date',ylabel='Observed time / default estimated time')
    for ax in axes:ax.axhline(1,color='#555',linestyle='--');ax.grid(alpha=.2)
    axes[0].legend(fontsize=8)
    save(fig,output,'pace_context',r,'A factor of 2 means twice the population-default time over the same accepted profile. This removes the fixed gradient/bike defaults, not all effects of terrain. Personal history and the live multiplier are intentionally absent. Other-bike rides of 1–2 h are omitted from this comparison plot.')
    r.heading('5. Does pace deteriorate later at comparable gradients?')
    proxy_ids={x['id'] for x in rides if x['proxy_eligible']}
    hour_labels=['1–2 h','2–3 h','3–4 h','4–6 h','6+ h']
    phase_summary=phase_stats(phases,'hour',hour_labels,proxy_ids)
    complete={id for id in proxy_ids if all(any(p['ride']==id and p['kind']=='quarter' and p['phase']==str(q) for p in phases) for q in (2,3,4))}
    fig,axes=plt.subplots(1,2,figsize=(12,4.5),constrained_layout=True)
    q=np.array([v[1] for v in phase_summary]);axes[0].plot(range(5),(q[:,1]-1)*100,'o-',color=COLORS['long'])
    axes[0].fill_between(range(5),(q[:,0]-1)*100,(q[:,2]-1)*100,color=COLORS['long'],alpha=.17)
    axes[0].set(xticks=range(5),xticklabels=[f'{s}\nn={n}' for s,(n,_) in zip(hour_labels,phase_summary)],xlabel='Observed moving hours into the ride',ylabel='Time/km change versus first hour (%)')
    quarter_values=[]
    for id in sorted(complete):
        values=[1]+[next(p['ratio'] for p in phases if p['ride']==id and p['kind']=='quarter' and p['phase']==str(q)) for q in (2,3,4)]
        quarter_values.append(values);axes[1].plot(range(4),(np.array(values)-1)*100,color='#777777',alpha=.13,linewidth=.7)
    if quarter_values:axes[1].plot(range(4),(np.median(quarter_values,axis=0)-1)*100,'o-',color=COLORS['long'],linewidth=2)
    axes[1].set(xticks=range(4),xticklabels=['First','Second','Third','Last'],xlabel=f'Quarter of observed ride time; same {len(complete)} rides',ylabel='Time/km change versus first quarter (%)')
    for ax in axes:ax.axhline(0,color='#444',linestyle='--');ax.grid(alpha=.2)
    save(fig,output,'within_ride_change',r,'Positive means slower. Compare only grade bins represented in both phases (2 percentage-point bins; at least 400 m each side and 1 km shared support). Left cohorts change with duration; right uses identical rides in all quarters. Main plot uses the earlier high-coverage proxy subset.')
    r.table(['Later phase','Supported rides','10th percentile change','Median change','90th percentile change'],[[label,n,*[f'{100*(x-1):+.1f}%' for x in quantile]] for label,(n,quantile) in zip(hour_labels,phase_summary)])
    r.paragraph('Matching removes much of the terrain-mix problem, but not changes within a grade bin, surface, wind, food, pauses, or companions. These plots can show a repeatable within-ride trend; they cannot attribute it specifically to fatigue. Unknown gaps are excluded from observed time.')
    r.heading('6. How well does the first hour predict the rest?')
    fig,axes=plt.subplots(1,2,figsize=(11,4.5),constrained_layout=True)
    for ax,checkpoint in zip(axes,(10,60)):
        a=[p for p in probes if p['mode']=='aggregate' and p['checkpoint']==checkpoint]
        early=np.array([p['early_factor'] for p in a]);late=np.array([p['later_factor'] for p in a])
        colors=[COLORS['long'] if next(x for x in rides if x['id']==p['ride'])['moving_minutes']>=360 else COLORS['short'] for p in a]
        ax.scatter(early,late,s=25,c=colors,alpha=.7)
        low,high=min(early.min(),late.min()),max(early.max(),late.max());ax.plot([low,high],[low,high],color='#555',linestyle='--')
        ax.set(title=f'First {checkpoint} observed minutes; n={len(a)}',xlabel='Early time / default time',ylabel='Remaining time / default time');ax.grid(alpha=.2)
    save(fig,output,'early_later_pace',r,'Each dot is a long evaluation ride. On the diagonal, its early aggregate pace factor would predict its remaining accepted time exactly. Orange denotes 6+ observed hours; green denotes shorter long rides. This comparison uses aggregate time ratios and the fixed gradient curve.')
    r.heading('7. Controlled probes: model/replay effects versus remaining variation')
    names={'reset':'Original: reset at unknown gaps','retain':'Keep live factor across gaps','aggregate':'Aggregate pace since departure'}
    table=[]
    for checkpoint in (10,30,60):
        for mode in ('reset','retain','aggregate'):
            m=summary['probes'][mode][str(checkpoint)]
            table.append([checkpoint,names[mode],m['n'],f"{m['mape']:.1f}%",f"{m['median_ape']:.1f}%",f"{m['p90_ape']:.1f}%",f"{m['mae_minutes']:.1f}",f"{m['bias_minutes']:+.1f}"])
    r.table(['Observed min','Probe','Rides','Mean APE','Median APE','90th percentile APE','Mean abs. min','Mean signed min'],table)
    r.paragraph(f"The original baseline reproduced {summary['verified_baseline_forecasts']} forecasts with maximum difference {summary['maximum_baseline_difference']:.3g} minutes. Both probes use the same 66 targets and accepted profile. They are exploratory comparisons on inspected data, not validated replacement algorithms.")
    r.paragraph('The retain probe changes only live resets at unknown gaps. The aggregate probe uses total observed moving time divided by total default predicted time since departure, then applies that factor to the remaining accepted route. It uses only past observations and does not reset at gaps. It changes both averaging and effective memory length, so any improvement cannot be attributed to averaging alone. It has no new outlier treatment or calibrated ranges.')
    r.paragraph('Residual error here is not a lower bound on what a better system could achieve. It still includes an imperfect gradient curve, missing road/trail context, unlabelled riding conditions, timing uncertainty, and limits of these simple probes. A broad distribution of individual section speeds also does not directly imply equally broad uncertainty in their total duration.')
    r.heading('Decision boundary')
    r.paragraph('These plots can identify useful structure and defects in the present design. They cannot establish that ETA is fundamentally unpredictable or that a more complex model would solve it. Before removing ETA as a feature, set an acceptable error for each use: whole-ride arrival, the next waypoint, and time to the next water source may need different horizons and displays. Any changed model needs new reserved rides for validation.')
    r.heading('Reproduction and privacy')
    r.paragraph(f"The exploration plan and computation/test hashes were recorded before its outputs. The original algorithm, frozen replay, and personal-ride files remain unchanged. Source summaries cover all exported own cycling recordings; timed analyses remove {len(source_rides)-len(rides)} overlaps and cannot recover unknown sections. Prepared arrays, plots, diagnostics, and this report remain local.")
    r.paragraph('Run python3 host/ride-time-prototype/komoot_explore.py, then python3 host/ride-time-prototype/komoot_explore_report.py from the repository root. The exploration refuses to overwrite an existing plan; regenerate figures without rerunning computation.')
    r.save(output)
    inputs=sorted(output.glob('*.svg'))+[output/name for name in ('context.json','interpretation.json') if (output/name).exists()]
    (output/'report-inputs.json').write_text(json.dumps(dict(report_source_sha256=digest(Path(__file__)),computation_run_sha256=digest(output/'run.json'),files={p.name:digest(p) for p in inputs}),indent=2)+'\n')
    print(str(output/'report.html'))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,default=SOURCE);p.add_argument('--output',type=Path,default=OUTPUT)
    a=p.parse_args();report(a.source,a.output)
