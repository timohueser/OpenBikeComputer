"""Progress preservation, delayed decisions, and forecast causality."""

from pathlib import Path
import sys
import unittest
import numpy as np
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from motion_filter import features,spans,stationary
from motion_replay import replay_ride
from final_model import Scalar
from model import Config


def points(t,x,y):
    return np.column_stack([t,np.asarray(y)/6371000*180/np.pi,np.asarray(x)/6371000*180/np.pi,np.zeros(len(t))])


def data(n):
    return dict(raw=np.array([np.full(n,.08),np.full(n,1/3),np.zeros(n)]),
        learn=np.ones(n,bool),reset=np.zeros(n,bool),unknown_minutes=np.zeros(n),pause_minutes=np.zeros(n))


class MotionFilterTests(unittest.TestCase):
    def test_preserves_slow_straight_and_curved_progress(self):
        t=np.arange(0,121,5.)
        for speed in (.3,.5,1,3):
            self.assertFalse(stationary(features(points(t,t*speed/3.6,np.zeros(len(t))))))
        angle=np.linspace(0,2*np.pi,len(t))
        self.assertFalse(stationary(features(points(t,8*np.cos(angle),8*np.sin(angle)))))

    def test_confined_jitter_is_excluded_but_short_span_is_not(self):
        t=np.arange(0,121,5.);p=points(t,np.sin(t)*2,np.cos(t)*2)
        self.assertTrue(stationary(features(p)))
        self.assertFalse(stationary(features(p[:-1])))

    def test_decisions_cover_intervals_and_do_not_bridge_gaps(self):
        t=np.arange(0,241,10.);p=points(t,np.sin(t),np.cos(t));d=data(len(t)-1)
        d['unknown_minutes'][10]=1
        s=spans(p,d)
        self.assertEqual(s[0],(0,10,False));self.assertEqual(s[1],(10,11,False))
        self.assertEqual([i for a,b,_ in s for i in range(a,b)],list(range(len(t)-1)))
        self.assertTrue(s[2][2])

    def test_future_positions_do_not_change_completed_decisions(self):
        t=np.arange(0,361,5.);p=points(t,np.sin(t),np.cos(t));d=data(len(t)-1)
        changed=p.copy();changed[t>120,1]+=.5
        a=[s for s in spans(p,d) if s[1]<=24];b=[s for s in spans(changed,d) if s[1]<=24]
        self.assertEqual(a,b)

    def test_future_times_cannot_change_one_hour_forecast(self):
        n=360;t=np.arange(n+1)*20.;p=points(t,t*4,np.zeros(n+1));d=data(n)
        cfg=Config();first=replay_ride(p,d,cfg,0,{k:Scalar(cfg) for k in ('raw','filtered')})[0]
        changed={k:v.copy() for k,v in d.items()};changed['raw'][1,270:]*=2
        later=p.copy();later[271:,0]+=np.arange(1,n-269)*20
        second=replay_ride(later,changed,cfg,0,{k:Scalar(cfg) for k in ('raw','filtered')})[0]
        a=[r for r in first if r['checkpoint']==60];b=[r for r in second if r['checkpoint']==60]
        self.assertEqual(len(a),4)
        for x,y in zip(a,b):
            self.assertEqual(x['predicted_minutes'],y['predicted_minutes'])
            self.assertEqual(x['wall_minutes'],y['wall_minutes'])
            self.assertNotEqual(x['actual_minutes'],y['actual_minutes'])


if __name__=='__main__':unittest.main()
