"""Fixed-checkpoint sensitivity to possible stationary GPS drift, not ground truth."""

import argparse
import json
from pathlib import Path

import numpy as np

from komoot_misses import OUTPUT, PREPARED
from komoot_misses_report import verify
from komoot_replay import configuration
from komoot_report import metrics
from long_data import digest
from model import Layout


def masks(data, spans, at):
    """Past exclusions use only candidate spans completed by the forecast boundary."""
    d,t,_=data['raw']
    last=np.flatnonzero((np.cumsum(t)<=at+1e-7)&(d>0))[-1]
    early=np.arange(len(t))<=last
    wall_end=np.cumsum(data['elapsed_minutes']);wall_start=wall_end-data['elapsed_minutes']
    past=np.zeros(len(t),bool);future=np.zeros(len(t),bool)
    for s in spans:
        m=(wall_start>=s['start_wall_min']-1e-7)&(wall_end<=s['end_wall_min']+1e-7)
        if s['end_wall_min']<=wall_end[last]+1e-7:past|=m&early
        future|=m&~early
    return early,past,future


def run(prepared,output):
    verify(output)
    motion_run=json.loads((output/'motion-run.json').read_text())
    if any(digest(output/n)!=h for n,h in motion_run['hashes'].items()):raise ValueError('Motion audit changed')
    root=Path(__file__).resolve().parent
    plan=dict(status='Exploratory change of motion proxy, not validated accuracy.',
        code_hashes={n:digest(root/n) for n in ('komoot_motion_sensitivity.py','tests/test_komoot_motion_sensitivity.py')},
        inputs={n:digest(output/n) for n in ('cases.json','motion.json')},
        methods=[
            'Same original forecast instants and same 66 rides. No re-selection or reset of the one-hour checkpoint after exclusions.',
            'Exclude only already-completed geometry-candidate spans from past aggregate time and default cost. No speed threshold and no future span may alter past observations.',
            'Remaining route/default cost unchanged. Compare with the original accepted remaining duration and an alternate outcome excluding candidate stationary spans. Future candidate labels affect only the alternate outcome.',
            'Cross both forecasts with both outcomes to expose target-definition effects. A change in error does not prove correct stop labels or actual moving-time accuracy.'
        ])
    with (output/'sensitivity-plan.json').open('x') as f:json.dump(plan,f,indent=2)
    motion=json.loads((output/'motion.json').read_text());cases=json.loads((output/'cases.json').read_text())
    cfg,offsets,_=configuration();rows=[];details=[]
    for c in cases:
        with np.load(prepared/(c['ride']+'.npz')) as f:data={k:f[k] for k in f.files}
        d,t,g=data['raw'];q=d*np.exp(Layout(cfg,gradient=False).initial_log(g)+offsets[c['bike']])
        early,past,future=masks(data,motion[c['ride']],c['at_minutes'])
        factor=float(t[early&~past].sum()/q[early&~past].sum())
        predicted=factor*q[~early].sum()
        clean_actual=float(t[~early&~future].sum())
        details.append(dict(ride=c['ride'],early_removed_min=float(t[past].sum()),later_removed_min=float(t[future].sum()),
            past_accepted_min=float(t[early&~past].sum()),clean_factor=factor,clean_prediction=float(predicted),
            clean_actual=clean_actual,clean_signed_percent=float(100*(predicted/clean_actual-1))))
        for mode,pred in [('original_aggregate',c['predicted_minutes']),('exclude_completed_candidates',predicted)]:
            for target,actual in [('original',c['actual_minutes']),('exclude_candidates',clean_actual)]:
                rows.append(dict(ride=c['ride'],mode=mode,target=target,predicted_minutes=float(pred),actual_minutes=actual,low_minutes=None,high_minutes=None))
    summary=[]
    for mode in ('original_aggregate','exclude_completed_candidates'):
        for target in ('original','exclude_candidates'):
            selected=[r for r in rows if r['mode']==mode and r['target']==target]
            ape=np.array([100*abs(r['predicted_minutes']/r['actual_minutes']-1) for r in selected])
            summary.append(dict(mode=mode,target=target,**metrics(selected),within5=int((ape<=5).sum()),within10=int((ape<=10).sum()),over20=int((ape>20).sum())))
    (output/'sensitivity.json').write_text(json.dumps(dict(summary=summary,details=details,rows=rows),indent=2)+'\n')
    (output/'sensitivity-run.json').write_text(json.dumps(dict(hashes={n:digest(output/n) for n in ('sensitivity-plan.json','sensitivity.json')}),indent=2)+'\n')
    print(json.dumps(summary,indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--prepared',type=Path,default=PREPARED);p.add_argument('--output',type=Path,default=OUTPUT)
    a=p.parse_args();run(a.prepared,a.output)
