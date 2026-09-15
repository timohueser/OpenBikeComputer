"""Chronological paired ETA replay with delayed motion decisions and a revised clock."""

import argparse
import json
import math
from pathlib import Path
import time

import numpy as np

from final_model import Scalar
from komoot_data import SOURCE,OUTPUT as PREPARED,read_gpx
from komoot_replay import blocks,configuration,verify
from komoot_report import metrics
from long_data import digest
from model import Layout,Live
from motion_filter import spans,features,stationary
from motion_review import OUTPUT as REVIEW

OUTPUT=Path('.artifacts/ride-time-motion-replay')
ROOT=Path(__file__).resolve().parent
MODES=('raw_live','filtered_live','raw_aggregate','filtered_aggregate')
CHECKPOINTS=(0,10,30,60)


def filtered(data,decisions):
    result={k:v.copy() for k,v in data.items()};mask=np.zeros(data['raw'].shape[1],bool)
    for a,b,stop in decisions:
        if stop:mask[a:b]=True
    result['raw'][:2,mask]=0;result['learn'][mask]=False;result['reset'][mask]=True
    after=np.r_[False,mask[:-1]]
    result['learn'][after]=False;result['reset'][after]=True
    return result,mask


def replay_ride(points,data,cfg,offset,learners,emit=True):
    decisions=spans(points,data)
    clean,mask=filtered(data,decisions)
    sets={'raw':data,'filtered':clean};live={k:Live(cfg) for k in sets}
    theta={k:l.theta for k,l in learners.items()}
    q=data['raw'][0]*np.exp(Layout(cfg,gradient=False).initial_log(data['raw'][2])+offset)
    suffix_q=np.r_[np.cumsum(q[::-1])[::-1],0.]
    future={k:np.r_[np.cumsum(v['raw'][1,::-1])[::-1],0.] for k,v in sets.items()}
    elapsed={k:0. for k in sets};cost={k:0. for k in sets}
    rows=[];next_slot=0

    def forecast(end):
        nonlocal next_slot
        while next_slot<len(CHECKPOINTS) and elapsed['filtered']+1e-8>=CHECKPOINTS[next_slot]:
            checkpoint=CHECKPOINTS[next_slot];next_slot+=1
            if not emit or future['filtered'][end]<=0 or suffix_q[end]<=0:continue
            for mode in MODES:
                kind='filtered' if mode.startswith('filtered') else 'raw'
                if mode.endswith('aggregate'):
                    if cost[kind]<=0:continue
                    factor=elapsed[kind]/cost[kind]
                else:factor=math.exp(theta[kind]+float(live[kind].u))
                rows.append(dict(mode=mode,checkpoint=checkpoint,at_filtered_minutes=elapsed['filtered'],
                    at_original_minutes=elapsed['raw'],wall_minutes=float((points[end,0]-points[0,0])/60),
                    actual_minutes=float(future['filtered'][end]),original_actual_minutes=float(future['raw'][end]),
                    predicted_minutes=float(suffix_q[end]*factor),theta=theta[kind],
                    low_minutes=None,high_minutes=None))
    forecast(0)
    for a,b,_ in decisions:
        for kind,v in sets.items():
            chunk={k:(value[:,a:b] if k=='raw' else value[a:b]) for k,value in v.items()}
            values=blocks(chunk,cfg,offset)
            for d,t,log,learn,_,reset in values.T:
                if reset:live[kind]=Live(cfg)
                if d<=0:continue
                base=float(d*math.exp(log));live[kind].observe(float(t),base*math.exp(theta[kind]))
                if learn:learners[kind].observe(float(log),float(t/d),float(d))
                elapsed[kind]+=float(t);cost[kind]+=base
        forecast(b)
    for kind,v in sets.items():
        if v['raw'][0].sum()>=1 and v['raw'][1].sum()>=5:learners[kind].finish()
        else:learners[kind].reset_ride()
        if learners[kind].rejections:raise ValueError('Persistent update rejected')
    return rows,dict(original_minutes=float(data['raw'][1].sum()),filtered_minutes=float(clean['raw'][1].sum()),
        removed_minutes=float(data['raw'][1,mask].sum()),removed_km=float(data['raw'][0,mask].sum()),
        stationary_spans=sum(s for _,_,s in decisions),release_spans=len(decisions),
        theta_after={k:l.theta for k,l in learners.items()})


def synthetic():
    rng=np.random.default_rng(20260915);rows=[];t=np.arange(0,121,5.)
    for sigma in (0.,1.,3.):
        for rho in (0.,.9):
            for speed in (0.,.1,.3,.5,1.,3.):
                count=0
                for _ in range(200):
                    noise=np.zeros((len(t),2));noise[0]=rng.normal(0,sigma,2)
                    for i in range(1,len(t)):noise[i]=rho*noise[i-1]+rng.normal(0,sigma*np.sqrt(1-rho*rho),2)
                    xy=noise+np.column_stack([t*speed/3.6,np.zeros(len(t))])
                    points=np.column_stack([t,xy[:,1]/6371000*180/np.pi,xy[:,0]/6371000*180/np.pi,np.zeros(len(t))])
                    count+=stationary(features(points))
                rows.append(dict(speed_kmh=speed,noise_sigma_m=sigma,noise_correlation=rho,n=200,excluded=count))
    return rows


def run(source,prepared,review,output):
    verify(source,prepared)
    reviewed=json.loads((review/'review.json').read_text())
    if reviewed['filter_sha256']!=digest(ROOT/'motion_filter.py') or reviewed['labels_sha256']!=digest(review/'labels.json'):raise ValueError('Reviewed filter or labels changed')
    output.mkdir(parents=True,exist_ok=True)
    plan=dict(status='Exploratory motion-proxy replay; not independent moving-time ground truth.',
        inputs={str(p.resolve()):digest(p) for p in (prepared/'protocol.json',review/'review.json',review/'labels.json')},
        source_hashes={n:digest(ROOT/n) for n in ('motion_filter.py','motion_replay.py','tests/test_motion_filter.py')},
        methods=[
            'Same retained chronological history and original long target IDs; no new target selection based on filtered duration or errors. Separate raw and filtered scalar histories, updated only after completed rides.',
            'Two-minute causal decisions; flush at unknown intervals/explicit pauses, retain incomplete spans. Stop requires <=30m extent and <=5m path between four smoothed position centroids. No speed threshold.',
            'Only forecast at completed decision spans, at/after 0/10/30/60 filtered moving minutes. Both observation policies share physical forecast positions and filtered outcomes. Report original outcomes separately.',
            'Same fixed gradient and bike curve and original route cost remain available for the remaining route; future stop classification affects outcomes only. Past blocks exclude classified stops. No retrospective correction of issued forecasts.',
            'Both policies use the same span-boundary block flushes. This paired release schedule differs from the original block-by-block replay. No baseline equality claim with earlier reported checkpoints.',
            'Compare frozen scalar/live and aggregate-since-departure. No pace parameter tuning, no uphill/fatigue trials, no calibrated range claims.',
            'Synthetic grid frozen before running: straight motion at 0/0.1/0.3/0.5/1/3 km/h, positional noise SD 0/1/3m with correlation 0/0.9, 200 replicates, seed 20260915. Slow-motion false exclusions must remain visible.'
        ])
    with (output/'plan.json').open('x') as f:json.dump(plan,f,indent=2)
    (output/'synthetic.json').write_text(json.dumps(synthetic(),indent=2)+'\n')
    protocol=json.loads((prepared/'protocol.json').read_text());included=set(protocol['cohort']['warmup']+protocol['cohort']['evaluation']);targets=set(protocol['long_proxy_targets'])
    rides=[r for r in json.loads((prepared/'rides.json').read_text()) if r['id'] in included]
    files={f['id']:f for f in json.loads((source/'manifest.json').read_text())['files']}
    cfg,offsets,_=configuration();learners={k:Scalar(cfg) for k in ('raw','filtered')};rows=[];audit=[];started=time.perf_counter()
    for i,ride in enumerate(rides):
        f=files[ride['id']];path=source/f['path']
        if digest(path)!=f['sha256']:raise ValueError('GPX changed')
        points,_=read_gpx(path)
        with np.load(prepared/(ride['id']+'.npz')) as f:data={k:f[k] for k in f.files}
        result,info=replay_ride(points,data,cfg,offsets[ride['bike']],learners,emit=ride['id'] in targets)
        rows.extend(dict(ride=ride['id'],**r) for r in result);audit.append(dict(ride=ride['id'],date=ride['date'],target=ride['id'] in targets,**info))
        if (i+1)%100==0:print(f'{i+1}/{len(rides)} rides; {time.perf_counter()-started:.1f}s',flush=True)
    summary=[]
    for checkpoint in CHECKPOINTS:
        for mode in MODES:
            a=[r for r in rows if r['mode']==mode and r['checkpoint']==checkpoint]
            if not a:continue
            for outcome in ('filtered','original'):
                selected=[dict(r,actual_minutes=r['original_actual_minutes']) if outcome=='original' else r for r in a]
                ape=np.array([100*abs(r['predicted_minutes']/r['actual_minutes']-1) for r in selected])
                summary.append(dict(checkpoint=checkpoint,mode=mode,outcome=outcome,**metrics(selected),within5=int((ape<=5).sum()),within10=int((ape<=10).sum())))
    for name,value in [('predictions.json',rows),('rides.json',audit),('summary.json',summary)]:
        (output/name).write_text(json.dumps(value,indent=2)+'\n')
    (output/'run.json').write_text(json.dumps(dict(runtime_seconds=time.perf_counter()-started,hashes={n:digest(output/n) for n in ('plan.json','synthetic.json','predictions.json','rides.json','summary.json')}),indent=2)+'\n')
    print(json.dumps([s for s in summary if s['checkpoint']==60 and s['outcome']=='filtered'],indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for n,d in [('source',SOURCE),('prepared',PREPARED),('review',REVIEW),('output',OUTPUT)]:p.add_argument('--'+n,type=Path,default=d)
    a=p.parse_args();run(a.source,a.prepared,a.review,a.output)
