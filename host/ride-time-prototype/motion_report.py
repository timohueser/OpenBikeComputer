"""Private report for motion review, synthetic limits, and chronological replay."""

import argparse
import base64
import json
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np

from komoot_report import Report
from long_data import digest
from motion_replay import OUTPUT,REVIEW

NAMES={'raw_live':'Original observations + live','filtered_live':'Filtered observations + live',
       'raw_aggregate':'Original observations + aggregate','filtered_aggregate':'Filtered observations + aggregate'}
COLORS={'raw_live':'#888888','filtered_live':'#7b65a6','raw_aggregate':'#c97843','filtered_aggregate':'#365d3c'}


def figure(fig,output,name,r,caption):
    fig.savefig(output/(name+'.svg'));fig.savefig(output/(name+'.png'),dpi=130);plt.close(fig)
    r.figure(output/(name+'.svg'),caption)


def report(review,output):
    run=json.loads((output/'run.json').read_text());plan=json.loads((output/'plan.json').read_text())
    if any(digest(output/n)!=h for n,h in run['hashes'].items()):raise ValueError('Results changed')
    root=Path(__file__).resolve().parent
    if any(digest(root/n)!=h for n,h in plan['source_hashes'].items()):raise ValueError('Replay sources changed')
    if any(digest(Path(n))!=h for n,h in plan['inputs'].items()):raise ValueError('Reviewed inputs changed')
    scores=json.loads((review/'review.json').read_text());synthetic=json.loads((output/'synthetic.json').read_text())
    summary=json.loads((output/'summary.json').read_text());rides=json.loads((output/'rides.json').read_text());rows=json.loads((output/'predictions.json').read_text())
    r=Report();r.heading('Motion filtering: what can these GPX recordings establish?',1)
    r.paragraph('Private exploratory study. One rider, geometry-only review, synthetic progress cases, and a chronological replay of the original retained history. The filter is not a validated physical stop detector. ETA scores remain conditional on a motion proxy and exclude recording gaps and breaks.')
    if (output/'interpretation.json').exists():
        i=json.loads((output/'interpretation.json').read_text());r.heading('Findings')
        for text in i['findings']:r.paragraph(text)
        r.heading('Decision');r.paragraph(i['decision'])
    r.heading('1. Rule and review')
    r.paragraph('Wait for approximately 120 seconds of uninterrupted GPS data. Exclude the span only if its bounding-box diagonal is at most 30 m and the path between four time-smoothed position centres is at most 5 m. The centres use 25 evenly timed interpolated positions. Short spans, unknown gaps, and explicit pauses do not acquire a new stop label. The rule has no riding-speed cutoff, but its geometry thresholds still impose limits on resolvable slow progress.')
    r.paragraph('The sample contains 48 spans on distinct rides: 24 development and 24 validation examples, split by ride hash. Each split includes eight confined traces, eight slow traces, and eight regular traces. One assistant labelled central spans from plots without classifier decisions or ETA errors shown. Any visible approach/departure or coherent loop counted as progress. Unclear drift versus movement stayed uncertain. There are no rider-confirmed pushing labels. The filter was fixed before selection and was not tuned after this review.')
    r.table(['Split','Visual label','Examples','Excluded by filter'],[[split,label,v['n'],v['excluded']] for split,c in scores['counts'].items() for label,v in c.items()])
    r.paragraph('Uncertain examples are reported separately, not counted as correct stops. Excluding an uncertain trace is an unresolved risk. Small, deliberately stratified samples cannot estimate population stop precision or recall. Even zero false exclusions among the clear-progress examples does not prove a zero false-exclusion rate.')
    r.heading('2. Synthetic slow progress and GPS noise')
    r.paragraph('Fixed grid: 200 replicates per condition, samples every five seconds, straight movement at 0/0.1/0.3/0.5/1/3 km/h, coordinate noise SD 0/1/3 m, and independent or correlated noise (rho 0.9). These noise assumptions are illustrative. The authored suite also checks a coherent small loop, delayed decisions, gap separation, and forecast invariance to future observations.')
    fig,axes=plt.subplots(1,2,figsize=(11,4),constrained_layout=True)
    speeds=(0,.1,.3,.5,1,3);sigmas=(0,1,3)
    for ax,rho in zip(axes,(0,.9)):
        matrix=np.array([[next(x['excluded']/2 for x in synthetic if x['speed_kmh']==v and x['noise_sigma_m']==sigma and x['noise_correlation']==rho) for v in speeds] for sigma in sigmas])
        im=ax.imshow(matrix,vmin=0,vmax=100,cmap='YlOrRd',aspect='auto')
        for i in range(3):
            for j in range(6):ax.text(j,i,f'{matrix[i,j]:g}%',ha='center',va='center',color='white' if matrix[i,j]>60 else 'black',fontsize=9)
        ax.set(xticks=range(6),xticklabels=speeds,yticks=range(3),yticklabels=sigmas,xlabel='True progress speed (km/h)',ylabel='Position noise SD (m)',title=f'Noise correlation {rho}')
    fig.colorbar(im,ax=axes,label='Spans excluded (%)')
    figure(fig,output,'synthetic',r,'At zero speed, exclusion is a correct stationary decision. At positive speed, exclusion removes real progress. Poor stationary detection in noisy conditions is also a failure; the filter can leave substantial drift in the data.')
    r.heading('3. Chronological replay with decisions available in time')
    targets=[x for x in rides if x['target']]
    r.table(['Item','Value'],[['Retained chronological rides',len(rides)],['Original long targets',len(targets)],['Targets now under two filtered hours',sum(x['filtered_minutes']<120 for x in targets)],['Rides with excluded time',sum(x['removed_minutes']>0 for x in rides)],['Excluded minutes, full history',f"{sum(x['removed_minutes'] for x in rides):.1f}"],['Excluded minutes, long targets',f"{sum(x['removed_minutes'] for x in targets):.1f}"],['Replay host runtime (seconds)',f"{run['runtime_seconds']:.1f}"]])
    r.paragraph('Separate original and filtered personal histories learn only after completed rides. Each motion span is committed at its endpoint; no forecasts use an incomplete span’s later classification. The revised moving clock advances only on retained observations. All four methods share forecast positions at the first completed span at/after 10, 30, and 60 filtered moving minutes. The comparison therefore does not reuse the earlier physical one-hour checkpoint.')
    r.paragraph('Remaining route costs and gradient/bike defaults stay fixed to the original accepted route proxy, including its residual GPS drift and profile errors. Future stop classifications affect scoring, not predicted route costs or past observations. Both policies flush observation blocks at the same release boundaries. This changes the batching schedule compared with the original pointwise replay, so numerical equality with earlier forecasts is not asserted. The experiment isolates the motion policy within this paired schedule; it is not a complete reconstruction of the true travelled route.')
    selected=[s for s in summary if s['outcome']=='filtered']
    r.table(['Filtered minute','Method','Rides','Mean APE','Median APE','90th percentile','Mean abs. min','Within 10%'],[[s['checkpoint'],NAMES[s['mode']],s['n'],f"{s['mape']:.1f}%",f"{s['median_ape']:.1f}%",f"{s['p90_ape']:.1f}%",f"{s['mae_minutes']:.1f}",f"{s['within10']}/{s['n']}"] for s in selected])
    fig,axes=plt.subplots(1,2,figsize=(11,4.3),constrained_layout=True)
    for mode in NAMES:
        a=[s for s in selected if s['mode']==mode and s['checkpoint']>0]
        axes[0].plot([s['checkpoint'] for s in a],[s['mape'] for s in a],'o-',color=COLORS[mode],label=NAMES[mode])
        errors=sorted(100*abs(x['predicted_minutes']/x['actual_minutes']-1) for x in rows if x['mode']==mode and x['checkpoint']==60)
        axes[1].step(errors,np.arange(1,len(errors)+1)/len(errors)*100,where='post',color=COLORS[mode])
    axes[0].set(xlabel='Filtered moving minutes at checkpoint',ylabel='Mean absolute percentage error (%)',xticks=[10,30,60]);axes[0].legend(fontsize=8)
    maximum=max(100*abs(x['predicted_minutes']/x['actual_minutes']-1) for x in rows if x['checkpoint']==60)
    axes[1].set(xlabel='Absolute error at one filtered hour (%)',ylabel='Rides within this error (%)',xlim=(0,maximum*1.05),ylim=(0,100))
    for ax in axes:ax.grid(alpha=.2)
    figure(fig,output,'accuracy',r,'Original and filtered observations have identical physical targets within this replay. A few large errors can dominate the mean; the cumulative distribution keeps the tail visible.')
    r.heading('4. Outcome-definition sensitivity')
    r.paragraph('For the same one-hour predictions, the table below also scores the original accepted-time outcome. It exposes dependence on the target definition. Neither outcome is independently verified moving time.')
    r.table(['Method','Outcome','Mean APE','Mean abs. min'],[[NAMES[s['mode']],s['outcome'],f"{s['mape']:.1f}%",f"{s['mae_minutes']:.1f}"] for s in summary if s['checkpoint']==60])
    r.heading('5. Validation trace sheets')
    r.paragraph('Blue square: central span start. Red cross: central span end. Colour advances through time. Grey lines give nearby context. Axes are metres. Labels were saved before scoring; uncertain examples remain uncertain.')
    for name in ('validation_1.png','validation_2.png'):
        encoded=base64.b64encode((review/name).read_bytes()).decode()
        r.body.append(f'<figure><img alt="Validation trace sheet" src="data:image/png;base64,{encoded}"></figure>')
        r.md.append(f'Validation sheet: {review.resolve()/name}')
    r.heading('Reproduction and limits')
    r.paragraph('Run motion_review.py select and render, save private labels.json, then score. Run motion_replay.py once, then motion_report.py to regenerate this report. Selection and replay refuse to replace plans. All personal outputs stay local. This is a Python research prototype; two-minute delayed observations and host NumPy storage are not an nRF54LM20 implementation or resource measurement.')
    r.save(output)
    extra=[output/'interpretation.json'] if (output/'interpretation.json').exists() else []
    inputs=[review/n for n in ('labels.json','review.json','validation_1.png','validation_2.png')]+extra+sorted(output.glob('*.svg'))
    (output/'report-inputs.json').write_text(json.dumps(dict(report_source_sha256=digest(Path(__file__)),run_sha256=digest(output/'run.json'),inputs={str(p.resolve()):digest(p) for p in inputs}),indent=2)+'\n')
    print(output/'report.html')


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--review',type=Path,default=REVIEW);p.add_argument('--output',type=Path,default=OUTPUT)
    a=p.parse_args();report(a.review,a.output)
