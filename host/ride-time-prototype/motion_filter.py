"""Conservative delayed GPS motion proxy; no minimum riding-speed threshold."""

import numpy as np

WINDOW_SECONDS = 120.
MAX_EXTENT_M = 30.
MAX_PROGRESS_M = 5.


def local_xy(points):
    lat=np.radians(points[:,1]);lon=np.radians(points[:,2])
    return np.column_stack([(lon-lon[0])*6371000*np.cos(lat[0]),(lat-lat[0])*6371000])


def features(points):
    xy=local_xy(points)
    times=points[:,0]-points[0,0]
    sample=np.column_stack([np.interp(np.linspace(0,times[-1],25),times,xy[:,k]) for k in (0,1)])
    centers=np.array([x.mean(axis=0) for x in np.array_split(sample,4)])
    return dict(seconds=float(times[-1]),extent_m=float(np.linalg.norm(np.ptp(xy,axis=0))),
                progress_m=float(np.linalg.norm(np.diff(centers,axis=0),axis=1).sum()))


def stationary(f):
    return f['seconds']>=WINDOW_SECONDS and f['extent_m']<=MAX_EXTENT_M and f['progress_m']<=MAX_PROGRESS_M


def spans(points,data):
    """Cover every interval once; release a decision only at the span's endpoint."""
    result=[];start=0
    for i in range(len(points)-1):
        if data['unknown_minutes'][i]>0 or data['pause_minutes'][i]>0:
            if start<i:result.append((start,i,False))
            result.append((i,i+1,False));start=i+1
        elif points[i+1,0]-points[start,0]>=WINDOW_SECONDS:
            result.append((start,i+1,stationary(features(points[start:i+2]))));start=i+1
    if start<len(points)-1:result.append((start,len(points)-1,False))
    return result
