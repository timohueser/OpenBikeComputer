"""Private, ride-separated visual review of a delayed motion proxy."""

import argparse
import hashlib
import json
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np

from komoot_data import SOURCE,OUTPUT as PREPARED,read_gpx
from komoot_replay import verify
from long_data import digest
from motion_filter import features,local_xy,spans,stationary

OUTPUT=Path('.artifacts/ride-time-motion-review')
ROOT=Path(__file__).resolve().parent


def order(value):return hashlib.sha256(('motion-review-v1:'+value).encode()).hexdigest()


def select(source,prepared,output):
    verify(source,prepared)
    output.mkdir(parents=True,exist_ok=True)
    plan=dict(status='Geometry review, not independently verified motion ground truth.',
        sampling='Ride-hash split into development and validation. Eight spans per split/stratum: confined, slow progress, regular. Hash selection, maximum one span per ride in each split. No ETA outcomes used.',
        labels='stationary-like, progress, uncertain; judge central coloured span using context. Record labels before scoring. No known pushing labels are available yet.',
        source_hash=digest(source/'manifest.json'),prepared_hash=digest(prepared/'rides.json'),
        source_files={n:digest(ROOT/n) for n in ('motion_filter.py','motion_review.py')})
    with (output/'plan.json').open('x') as f:json.dump(plan,f,indent=2)
    inventory=json.loads((source/'manifest.json').read_text());files={f['id']:f for f in inventory['files']}
    pool=[]
    for ride in json.loads((prepared/'rides.json').read_text()):
        file=files[ride['id']];path=source/file['path']
        if digest(path)!=file['sha256']:raise ValueError('GPX changed')
        points,_=read_gpx(path)
        with np.load(prepared/(ride['id']+'.npz')) as f:data={k:f[k] for k in f.files}
        split='development' if int(order(ride['id'])[:8],16)%2==0 else 'validation'
        for a,b,_ in spans(points,data):
            if points[b,0]-points[a,0]<120 or data['unknown_minutes'][a:b].any() or data['pause_minutes'][a:b].any():continue
            f=features(points[a:b+1]);speed=float(60*data['raw'][0,a:b].sum()/max(data['raw'][1,a:b].sum(),1e-10))
            stratum='confined' if f['extent_m']<=30 else 'slow' if speed<5 else 'regular'
            pool.append(dict(ride=ride['id'],start=a,end=b,split=split,stratum=stratum,**f))
    chosen=[]
    for split in ('development','validation'):
        used=set()
        for stratum in ('confined','slow','regular'):
            candidates=sorted([r for r in pool if r['split']==split and r['stratum']==stratum],key=lambda r:order(r['ride']+':'+str(r['start'])))
            count=0
            for row in candidates:
                if row['ride'] in used:continue
                chosen.append(row);used.add(row['ride']);count+=1
                if count==8:break
            if count!=8:raise ValueError('Insufficient distinct rides')
    for split in ('development','validation'):
        selected=sorted([r for r in chosen if r['split']==split],key=lambda r:order('display:'+r['ride']))
        for i,row in enumerate(selected,1):row['case']=split[0].upper()+str(i).zfill(2)
    (output/'catalogue.json').write_text(json.dumps(chosen,indent=2)+'\n')
    print('Selected',len(chosen),'spans on distinct rides')


def render(source,output):
    catalogue=json.loads((output/'catalogue.json').read_text())
    for split in ('development','validation'):
        rows=sorted([r for r in catalogue if r['split']==split],key=lambda r:r['case'])
        for page in range(2):
            fig,axes=plt.subplots(4,3,figsize=(13,15),constrained_layout=True)
            for ax,row in zip(axes.flat,rows[page*12:(page+1)*12]):
                points,_=read_gpx(source/'downloads'/(row['ride']+'.gpx'))
                a,b=row['start'],row['end'];lo=max(0,int(np.searchsorted(points[:,0],points[a,0]-60)));hi=min(len(points),int(np.searchsorted(points[:,0],points[b,0]+60)))
                xy=local_xy(points[lo:hi]);aa,bb=a-lo,b-lo
                ax.plot(xy[:,0],xy[:,1],color='#ccc',linewidth=1)
                middle=xy[aa:bb+1];ax.plot(middle[:,0],middle[:,1],color='#777',linewidth=.7)
                ax.scatter(middle[:,0],middle[:,1],c=np.linspace(0,1,len(middle)),s=10,cmap='viridis')
                ax.scatter(*middle[0],marker='s',color='blue',s=30);ax.scatter(*middle[-1],marker='x',color='red',s=45)
                ax.set(title=row['case'],xlabel='East (m)',ylabel='North (m)',aspect='equal');ax.grid(alpha=.2)
                pad=np.maximum(5,np.ptp(middle,axis=0)*.2)
                ax.set_xlim(middle[:,0].min()-pad[0],middle[:,0].max()+pad[0]);ax.set_ylim(middle[:,1].min()-pad[1],middle[:,1].max()+pad[1])
            fig.savefig(output/f'{split}_{page+1}.png',dpi=135);plt.close(fig)
    print('Rendered four blind sheets; blue square=start, red cross=end, grey=context')


def score(output):
    labels=json.loads((output/'labels.json').read_text())
    catalogue=json.loads((output/'catalogue.json').read_text())
    if set(labels)!={r['case'] for r in catalogue}:raise ValueError('Missing/extra labels')
    if not set(labels.values())<={'stationary-like','progress','uncertain'}:raise ValueError('Invalid visual label')
    result={}
    for split in ('development','validation'):
        rows=[r for r in catalogue if r['split']==split];counts={}
        for label in ('stationary-like','progress','uncertain'):
            selected=[r for r in rows if labels[r['case']]==label]
            counts[label]=dict(n=len(selected),excluded=sum(stationary(r) for r in selected))
        result[split]=counts
    (output/'review.json').write_text(json.dumps(dict(counts=result,labels_sha256=digest(output/'labels.json'),catalogue_sha256=digest(output/'catalogue.json'),filter_sha256=digest(ROOT/'motion_filter.py')),indent=2)+'\n')
    print(json.dumps(result,indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('action',choices=('select','render','score'))
    for n,d in [('source',SOURCE),('prepared',PREPARED),('output',OUTPUT)]:p.add_argument('--'+n,type=Path,default=d)
    a=p.parse_args()
    if a.action=='select':select(a.source,a.prepared,a.output)
    elif a.action=='render':render(a.source,a.output)
    else:score(a.output)
