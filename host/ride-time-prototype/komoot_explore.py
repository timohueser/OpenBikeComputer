"""Exploratory ride variability and controlled replay probes on private Komoot data."""

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import time

import numpy as np

from endurance import Correction
from final_model import Scalar
from komoot_data import OUTPUT as PREPARED, SOURCE
from komoot_replay import blocks, can_train, configuration, replay, verify
from komoot_report import load, metrics
from long_data import digest

OUTPUT = Path('.artifacts/ride-time-komoot-exploration')
ROOT = Path(__file__).resolve().parent
GRADE_EDGES = np.array([-20, -10, -6, -3, -1, 1, 3, 6, 10, 20])/100


def windows(data, width=.2):
    """About 200 m windows; preserve short tails >=50 m and never cross reset masks."""
    d, t, g = data['raw']
    elapsed = np.r_[0., np.cumsum(t)]
    rows, pending = [], []

    def flush():
        if not pending:
            return
        ix = np.asarray(pending)
        distance, minutes = float(d[ix].sum()), float(t[ix].sum())
        if distance >= .05 and minutes > 0:
            rows.append([distance, minutes, float(d[ix] @ g[ix])/distance,
                         float(elapsed[ix[0]]), float(elapsed[ix[-1]+1])])
        pending.clear()

    distance = 0.
    for i in range(len(d)):
        if data['reset'][i]:
            flush(); distance = 0.
        if d[i] > 0 and t[i] > 0:
            pending.append(i); distance += d[i]
        if distance >= width or data['reset'][i]:
            flush(); distance = 0.
    flush()
    return np.array(rows, dtype=float).reshape(-1, 5)


def grade_speeds(w, minimum_km=.5):
    """One aggregate speed per ride and grade bin, with explicit distance support."""
    values = []
    for low, high in zip(GRADE_EDGES[:-1], GRADE_EDGES[1:]):
        m = (w[:, 2] >= low) & (w[:, 2] < high)
        d, t = w[m, 0].sum(), w[m, 1].sum()
        values.append(60*d/t if d >= minimum_km and t > 0 else np.nan)
    return np.array(values)


def matched_ratio(w, reference, later):
    """Compare phase pace within narrow grade bins; balance their distance support."""
    log_ratios, support = [], []
    # 2 percentage-point bins; require at least 400 m in each phase/bin.
    for low in np.arange(-.2, .2, .02):
        grade = (w[:, 2] >= low) & (w[:, 2] < low+.02)
        a, b = w[reference & grade], w[later & grade]
        da, db = a[:, 0].sum(), b[:, 0].sum()
        if da >= .4 and db >= .4:
            log_ratios.append(np.log((b[:, 1].sum()/db)/(a[:, 1].sum()/da)))
            support.append(min(da, db))
    if sum(support) < 1:
        return None
    return dict(ratio=float(np.exp(np.average(log_ratios, weights=support))), matched_km=sum(support))


def early_forecasts(values, checkpoints=(10, 30, 60)):
    """Prospective probe: total observed time/default time from the start so far."""
    d, t, logs, *_ = values
    q = d*np.exp(logs)
    at = np.r_[0., np.cumsum(t)]
    known_q = np.r_[0., np.cumsum(q)]
    rows = []
    for checkpoint in checkpoints:
        possible = np.flatnonzero((at[:-1] >= checkpoint) & (d > 0))
        if not len(possible):
            continue
        i = int(possible[0])
        if known_q[i] <= 0:
            continue
        ratio = at[i]/known_q[i]
        remaining_q = q[i:].sum()
        actual = t[i:].sum()
        rows.append(dict(checkpoint=checkpoint, at_minutes=float(at[i]),
            actual_minutes=float(actual), predicted_minutes=float(remaining_q*ratio),
            early_factor=float(ratio), later_factor=float(actual/remaining_q),
            low_minutes=None, high_minutes=None))
    return rows


def describe(values):
    a = np.array(values, dtype=float)
    a = a[np.isfinite(a)]
    return dict(n=len(a), q10=float(np.quantile(a, .1)), median=float(np.median(a)),
                q90=float(np.quantile(a, .9))) if len(a) else dict(n=0)


def run(source, prepared, output):
    verify(source, prepared)
    original_run = json.loads((prepared/'run.json').read_text())
    if any(digest(prepared/name) != value for name, value in original_run['hashes'].items()):
        raise ValueError('Original replay outputs changed')
    output.mkdir(parents=True, exist_ok=True)
    protocol = dict(created_at_utc=datetime.now(timezone.utc).isoformat(),
        status='Exploratory diagnostics on already inspected data. No new generalization claim.',
        source_protocol_sha256=digest(prepared/'protocol.json'),
        original_predictions_sha256=digest(prepared/'predictions.csv'),
        methods=['Inventory: all 514 source summaries; timed-section analysis: 501 retained non-overlapping rides, including rides outside the earlier proxy cohort.',
            'About 200 m uninterrupted windows; discard tails below 50 m for plots only. Speed=sum(distance)/sum(time); gradient is distance-weighted mean of the prepared trailing-200m gradient.',
            'Speed-versus-grade bands: one aggregate speed per ride/bin, at least 0.5 km support; equal ride weighting. Full 10-90% distributions are descriptive, not confidence intervals.',
            'Matched within-ride change: 2 percentage-point grade bins, >=0.4 km each phase/bin, >=1 km total shared support; geometric mean pace ratios weighted by minimum phase/bin distance.',
            'Duration strata are descriptive and use an observed outcome; distance strata are also shown. No solo/partner or luggage labels are inferred.',
            'Two fixed causal probes: retain live pace at unknown gaps; or predict from aggregate observed-time/default-time ratio since departure. Same 66 long targets; original baseline must reproduce exactly. No range claims for probes.',
            'Future outcomes enter diagnostic summaries, never the two probes forecasts. Existing personal learning, priors, and prepared observations remain unchanged.'],
        code_hashes={name:digest(ROOT/name) for name in ('komoot_explore.py','tests/test_komoot_explore.py')})
    with (output/'plan.json').open('x') as f:json.dump(protocol,f,indent=2)
    source_manifest = json.loads((source/'manifest.json').read_text())
    rides = json.loads((prepared/'rides.json').read_text())
    plan = json.loads((prepared/'protocol.json').read_text())
    target_ids = set(plan['long_proxy_targets'])
    included = set(plan['cohort']['warmup']+plan['cohort']['evaluation'])
    cfg, offsets, reference = configuration()
    originals = {(r['ride'],r['checkpoint']):r for r in load(prepared)
                 if r['scenario']=='continuous' and r['mode']=='baseline'}
    devices = {mode:(Scalar(cfg),Correction('baseline'),None) for mode in ('reset','retain')}
    observations, stats, phase_rows, probe_rows = [], [], [], []
    reproduced, maximum_difference = 0, 0.
    started = time.perf_counter()
    for number, ride in enumerate(rides):
        with np.load(prepared/(ride['id']+'.npz')) as archive:
            data = {k:archive[k] for k in archive.files}
        w = windows(data)
        b = blocks(data,cfg,offsets[ride['bike']])
        q = b[0]*np.exp(b[2])
        s = dict(ride, window_km=float(w[:,0].sum()),
                 grade_speeds=grade_speeds(w).tolist(),
                 time_factor=float(b[1].sum()/q.sum()) if q.sum()>0 else None)
        stats.append(s)
        if len(w):observations.append(np.column_stack([np.full(len(w),number),w]))
        if ride['moving_minutes'] >= 120:
            mid = (w[:,3]+w[:,4])/2
            ref = mid<60
            for name,lo,hi in [('1–2 h',60,120),('2–3 h',120,180),('3–4 h',180,240),('4–6 h',240,360),('6+ h',360,float('inf'))]:
                matched = matched_ratio(w,ref,(mid>=lo)&(mid<hi))
                if matched:phase_rows.append(dict(ride=ride['id'],kind='hour',phase=name,**matched))
            frac=mid/ride['moving_minutes']
            for quarter in (1,2,3):
                matched=matched_ratio(w,frac<.25,(frac>=quarter/4)&(frac<(quarter+1)/4))
                if matched:phase_rows.append(dict(ride=ride['id'],kind='quarter',phase=str(quarter+1),**matched))
        if ride['id'] in included:
            for mode in devices:
                v=b.copy()
                if mode=='retain':v[5]=0
                result=replay(v,cfg,devices[mode],train=can_train(ride),calibrate=False,emit=ride['id'] in target_ids)
                for row in result:
                    if mode=='reset':
                        old=originals[(ride['id'],row['checkpoint'])]
                        diff=abs(row['predicted_minutes']-old['predicted_minutes'])
                        maximum_difference=max(maximum_difference,diff);reproduced+=1
                        if diff>1e-8 or abs(row['actual_minutes']-old['actual_minutes'])>1e-8:
                            raise ValueError('Baseline reproduction failed')
                    if row['checkpoint'] in (10,30,60):probe_rows.append(dict(mode=mode,ride=ride['id'],**row))
            if ride['id'] in target_ids:
                probe_rows += [dict(mode='aggregate',ride=ride['id'],**row) for row in early_forecasts(b)]
        if (number+1)%100==0:print(f"{number+1}/{len(rides)} rides; {time.perf_counter()-started:.1f}s",flush=True)
    summary = dict(source_rides=len(source_manifest['rides']),retained=len(rides),
        source_sports={s:sum(r['sport']==s for r in source_manifest['rides']) for s in ('mtb','racebike','touringbicycle')},
        window_km=sum(s['window_km'] for s in stats),accepted_km=sum(s['distance_km'] for s in stats),
        verified_baseline_forecasts=reproduced,maximum_baseline_difference=maximum_difference,
        probes={mode:{str(c):metrics([r for r in probe_rows if r['mode']==mode and r['checkpoint']==c])
                      for c in (10,30,60)} for mode in devices.keys()|{'aggregate'}},
        phases={kind:{phase:describe([r['ratio'] for r in phase_rows if r['kind']==kind and r['phase']==phase])
                      for phase in sorted({r['phase'] for r in phase_rows if r['kind']==kind})} for kind in ('hour','quarter')},
        runtime_seconds=time.perf_counter()-started)
    np.savez_compressed(output/'windows.npz',windows=np.concatenate(observations))
    for name,value in [('rides.json',stats),('phases.json',phase_rows),('probes.json',probe_rows),('summary.json',summary)]:
        (output/name).write_text(json.dumps(value,indent=2)+'\n')
    with (output/'run.json').open('x') as f:
        json.dump(dict(hashes={name:digest(output/name) for name in ('plan.json','windows.npz','rides.json','phases.json','probes.json','summary.json')}),f,indent=2)
    print(json.dumps(summary,indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source',type=Path,default=SOURCE)
    p.add_argument('--prepared',type=Path,default=PREPARED)
    p.add_argument('--output',type=Path,default=OUTPUT)
    a=p.parse_args();run(a.source,a.prepared,a.output)
