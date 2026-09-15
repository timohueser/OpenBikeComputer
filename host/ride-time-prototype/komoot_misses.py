"""Private diagnosis of remaining ETA misses and fixed-distance horizons."""

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path

import numpy as np

from komoot_data import OUTPUT as PREPARED, SOURCE
from komoot_explore import OUTPUT as EXPLORATION
from komoot_replay import configuration, verify
from komoot_report import metrics
from long_data import digest
from model import Layout

OUTPUT = Path('.artifacts/ride-time-komoot-misses')
EDGES = np.array([-np.inf, -.06, -.03, -.01, .01, .03, .06, .10, np.inf])
LABELS = ('Below −6%', '−6 to −3%', '−3 to −1%', '−1 to +1%', '+1 to +3%', '+3 to +6%', '+6 to +10%', 'Above +10%')


def time_weights(t, start, end):
    """Fraction of each interval inside an observed-moving-time window."""
    stop = np.cumsum(t)
    overlap = np.maximum(0, np.minimum(stop, end)-np.maximum(stop-t, start))
    return np.divide(overlap, t, out=np.zeros_like(t), where=t>0)


def distance_weights(d, start, km):
    """Future accepted-distance horizon, with proportional final interval."""
    ahead = d.copy(); ahead[:start] = 0
    if ahead.sum()+1e-9 < km:
        return None
    before = np.cumsum(ahead)-ahead
    take = np.minimum(ahead, np.maximum(0, km-before))
    return np.divide(take, d, out=np.zeros_like(d), where=d>0)


def factors(d, t, q, g, early):
    """Past-only aggregate and supported grade-specific factors."""
    scalar = float(t[early].sum()/q[early].sum())
    bins = np.clip(np.searchsorted(EDGES, g, side='right')-1, 0, len(LABELS)-1)
    by_grade = np.full(len(LABELS), scalar)
    support = np.zeros(len(LABELS))
    for k in range(len(LABELS)):
        mask = early & (bins==k)
        support[k] = d[mask].sum()
        if support[k] >= .4:
            by_grade[k] = t[mask].sum()/q[mask].sum()
    return scalar, by_grade, bins, support


def diagnose(data, cfg, offset, forecast):
    d, t, g = data['raw']
    q = d*np.exp(Layout(cfg, gradient=False).initial_log(g)+offset)
    at = forecast['at_minutes']
    stop = np.cumsum(t)
    last_observed = np.flatnonzero((stop <= at+1e-7) & (d>0))[-1]
    early = np.arange(len(d)) <= last_observed
    future = ~early
    if abs(t[early].sum()-at)>1e-7:
        raise ValueError('Checkpoint is not a prepared-interval boundary')
    scalar, grade, bins, support = factors(d,t,q,g,early)
    if abs(scalar*q[future].sum()-forecast['predicted_minutes'])>1e-6:
        raise ValueError('Aggregate forecast reproduction failed')
    recent = time_weights(t,max(0,at-30),at)
    recent_factor = float((t@recent)/(q@recent))
    original_factor = forecast['original_minutes']/q[future].sum()
    factors_by_mode = dict(original=np.full(len(d),original_factor),aggregate=np.full(len(d),scalar),
                           recent30=np.full(len(d),recent_factor),early_grade=grade[bins])
    rows=[]
    start=int(np.flatnonzero(future & (d>0))[0])
    for horizon in ('remaining',5,10,20):
        weights=future.astype(float) if horizon=='remaining' else distance_weights(d,start,horizon)
        if weights is None:continue
        selected=np.flatnonzero(weights>0)
        # A gap between the checkpoint and the endpoint invalidates a contiguous target.
        span=slice(int(np.flatnonzero(future)[0]),int(selected[-1])+1)
        contiguous=not bool(np.any(data['unknown_minutes'][span]>0))
        for mode, f in factors_by_mode.items():
            rows.append(dict(mode=mode,horizon=str(horizon),contiguous=contiguous,
                actual_minutes=float(t@weights),predicted_minutes=float((q*f)@weights),
                low_minutes=None,high_minutes=None))
    terrain=[]
    for k,label in enumerate(LABELS):
        mask=future & (bins==k)
        cost=float(q[mask].sum());actual=float(t[mask].sum())
        terrain.append(dict(grade=label,early_km=float(support[k]),future_km=float(d[mask].sum()),
            early_factor=float(grade[k]),supported=bool(support[k]>=.4),
            later_factor=actual/cost if cost>0 else None,
            scalar_error=float(scalar*cost-actual),
            mix_component=float((scalar-grade[k])*cost),
            within_component=float(grade[k]*cost-actual)))
    trace=[]
    for lo in np.arange(0,t.sum(),10):
        w=time_weights(t,float(lo),float(lo+10))
        if q@w>0:
            trace.append(dict(minute=float(min(lo+5,t.sum())),factor=float((t@w)/(q@w)),
                speed=float(60*(d@w)/(t@w)),grade=float((d*g)@w/(d@w)),
                climb_time_fraction=float((t*(g>=.03))@w/(t@w))))
    return dict(at_minutes=at,early_factor=scalar,recent_factor=recent_factor,
        later_factor=float(t[future].sum()/q[future].sum()),terrain=terrain,trace=trace,
        unsupported_future_cost_fraction=float(q[future & (support[bins]<.4)].sum()/q[future].sum()),
        unknown_minutes=float(data['unknown_minutes'].sum()),
        slow_time_fraction=float(t[(d>0)&(60*d/np.maximum(t,1e-30)<4)].sum()/t.sum())),rows


def summarize(rows):
    result=[]
    for horizon in ('remaining','5','10','20'):
        for scope in ('accepted','contiguous'):
            for mode in ('original','aggregate','recent30','early_grade'):
                a=[r for r in rows if r['horizon']==horizon and r['mode']==mode and (scope=='accepted' or r['contiguous'])]
                m=metrics(a)
                if a:
                    ape=np.array([100*abs(r['predicted_minutes']/r['actual_minutes']-1) for r in a])
                    m.update(within5=int((ape<=5).sum()),within10=int((ape<=10).sum()),over20=int((ape>20).sum()))
                result.append(dict(horizon=horizon,scope=scope,mode=mode,**m))
    return result


def run(source,prepared,exploration,output):
    verify(source,prepared)
    for folder in (prepared,exploration):
        manifest=json.loads((folder/'run.json').read_text())
        if any(digest(folder/name)!=h for name,h in manifest['hashes'].items()):
            raise ValueError('Frozen input changed')
    root=Path(__file__).resolve().parent
    plan=dict(created_at_utc=datetime.now(timezone.utc).isoformat(),status='Exploratory; previously inspected data.',
        input_hashes={str((exploration/name).resolve()):digest(exploration/name) for name in ('probes.json','rides.json')},
        code_hashes={name:digest(root/name) for name in ('komoot_misses.py','tests/test_komoot_misses.py')},
        methods=[
            'Same long targets, first original block boundary after 60 accepted moving minutes. Reproduce previous aggregate forecasts from prepared intervals.',
            'Original and aggregate factors plus two fixed diagnostic probes: aggregate last 30 moving minutes; separate first-hour factors in eight grade bins with >=400m support and scalar fallback. No tuning or prediction-range claims.',
            'Exact signed-error decomposition: scalar minus early-grade prediction is grade-factor/mix sensitivity; early-grade prediction minus outcome is within-bin change plus unsupported terrain. Neither term identifies physical causes.',
            'Remaining ride and fixed 5/10/20 accepted km; require full distance, prorate only final interval. Report same-target mode comparisons and separately targets with no unknown interval before endpoint. Pauses remain excluded.',
            'Detailed cases: union of 10 largest absolute percentage errors and 5 largest absolute minute errors for the aggregate one-hour forecast. Also report the whole cohort; do not drop outliers.',
            'Show +/-5% and +/-10% success counts and >20% errors. No assertion that a single-digit mean implies reliability.'
        ])
    output.mkdir(parents=True,exist_ok=True)
    with (output/'plan.json').open('x') as f:json.dump(plan,f,indent=2)
    probes=json.loads((exploration/'probes.json').read_text())
    original={r['ride']:r for r in probes if r['mode']=='reset' and r['checkpoint']==60}
    targets=[r for r in probes if r['mode']=='aggregate' and r['checkpoint']==60]
    rides={r['id']:r for r in json.loads((exploration/'rides.json').read_text())}
    cfg,offsets,_=configuration();cases=[];rows=[]
    for forecast in targets:
        ride=rides[forecast['ride']]
        with np.load(prepared/(ride['id']+'.npz')) as f:data={k:f[k] for k in f.files}
        case, predictions=diagnose(data,cfg,offsets[ride['bike']],dict(forecast,original_minutes=original[ride['id']]['predicted_minutes']))
        error=forecast['predicted_minutes']-forecast['actual_minutes']
        case.update(ride=ride['id'],date=ride['date'],bike=ride['bike'],km=ride['distance_km'],
            moving_minutes=ride['moving_minutes'],actual_minutes=forecast['actual_minutes'],
            predicted_minutes=forecast['predicted_minutes'],error_minutes=error,signed_percent=100*error/forecast['actual_minutes'])
        cases.append(case)
        rows.extend(dict(ride=ride['id'],**r) for r in predictions)
    chosen={c['ride'] for c in sorted(cases,key=lambda c:abs(c['signed_percent']),reverse=True)[:10]}
    chosen.update(c['ride'] for c in sorted(cases,key=lambda c:abs(c['error_minutes']),reverse=True)[:5])
    for c in cases:c['detailed']=c['ride'] in chosen
    for name,value in [('cases.json',cases),('predictions.json',rows),('summary.json',summarize(rows))]:
        (output/name).write_text(json.dumps(value,indent=2)+'\n')
    (output/'run.json').write_text(json.dumps(dict(hashes={n:digest(output/n) for n in ('plan.json','cases.json','predictions.json','summary.json')}),indent=2)+'\n')
    print(json.dumps([s for s in summarize(rows) if s['scope']=='accepted'],indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for name,default in [('source',SOURCE),('prepared',PREPARED),('exploration',EXPLORATION),('output',OUTPUT)]:
        p.add_argument('--'+name,type=Path,default=default)
    a=p.parse_args();run(a.source,a.prepared,a.exploration,a.output)
