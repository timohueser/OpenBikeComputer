"""Geometry-only audit of possible stationary GPS drift; never changes ride targets."""

import numpy as np


def stationary_candidates(points, data, at):
    """Disjoint ~120 s spans inside a 30 m box; suggest inspection, not stop labels."""
    lat=np.radians(points[:,1]);lon=np.radians(points[:,2])
    xy=np.column_stack([(lon-lon[0])*6371000,(lat-lat[0])*6371000])
    elapsed=points[:,0]-points[0,0]
    accepted=data['raw'][1]
    moving_stop=np.cumsum(accepted)
    last=np.flatnonzero((moving_stop<=at+1e-7)&(data['raw'][0]>0))[-1]
    early=np.arange(len(accepted))<=last
    rows=[];start=0
    for i in range(len(accepted)):
        if data['unknown_minutes'][i]>0 or data['pause_minutes'][i]>0:
            start=i+1
            continue
        if elapsed[i+1]-elapsed[start]<120:
            continue
        window=xy[start:i+2].copy()
        window[:,0]*=np.cos(lat[start:i+2].mean())
        extent=float(np.linalg.norm(np.ptp(window,axis=0)))
        time=float(accepted[start:i+1].sum())
        if extent<=30 and time>=1:
            mask=np.zeros(len(accepted),bool);mask[start:i+1]=True
            rows.append(dict(start_wall_min=float(elapsed[start]/60),end_wall_min=float(elapsed[i+1]/60),
                start_moving_min=float(moving_stop[start]-accepted[start]),extent_m=extent,
                net_m=float(np.linalg.norm(window[-1]-window[0])),
                recorded_path_m=float(data['recorded_distance_km'][start:i+1].sum()*1000),
                early_accepted_min=float(accepted[mask&early].sum()),later_accepted_min=float(accepted[mask&~early].sum())))
        start=i+1
    return rows
