"""Geometry audit must preserve progress and must not bridge unknown time."""

from pathlib import Path
import sys
import unittest
import numpy as np
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from komoot_motion import stationary_candidates


class MotionAuditTests(unittest.TestCase):
    def inputs(self,progress):
        points=np.zeros((25,4));points[:,0]=np.arange(25)*10
        points[:,1]=np.arange(25)*.0001 if progress else np.sin(np.arange(25))*.00001
        data=dict(raw=np.array([np.ones(24)*.001,np.ones(24)/6,np.zeros(24)]),
                  unknown_minutes=np.zeros(24),pause_minutes=np.zeros(24),recorded_distance_km=np.ones(24)*.001)
        return points,data

    def test_confined_drift_flagged_but_progress_not_flagged(self):
        points,data=self.inputs(False)
        rows=stationary_candidates(points,data,2)
        self.assertEqual(len(rows),2)
        self.assertAlmostEqual(rows[0]['early_accepted_min'],2)
        self.assertAlmostEqual(rows[1]['later_accepted_min'],2)
        points,data=self.inputs(True)
        self.assertEqual(stationary_candidates(points,data,2),[])

    def test_unknown_interval_breaks_stationary_window(self):
        points,data=self.inputs(False)
        data['unknown_minutes'][10]=1
        rows=stationary_candidates(points,data,2)
        self.assertEqual(len(rows),1)
        self.assertGreater(rows[0]['start_wall_min'],1)


if __name__=='__main__':unittest.main()
