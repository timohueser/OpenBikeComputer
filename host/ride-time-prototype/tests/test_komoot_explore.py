"""Support, causality, and gap contracts for the exploratory diagnostics."""

from pathlib import Path
import sys
import unittest

import numpy as np

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from komoot_explore import early_forecasts,grade_speeds,matched_ratio,windows


class ExplorationTests(unittest.TestCase):
    def test_windows_never_bridge_unknown_gaps(self):
        data=dict(raw=np.array([[.1,.1,0,.04,.2],[1,1,0,1,2],[0,0,0,0,0]]),
                  reset=np.array([0,0,1,0,0],bool))
        w=windows(data)
        np.testing.assert_allclose(w[:,:2],[[.2,2],[.24,3]])
        np.testing.assert_allclose(w[:,3:],[[0,2],[2,5]])

    def test_grade_bin_speed_uses_total_time_and_requires_support(self):
        w=np.array([[.3,1,0,0,1],[.3,3,0,1,4],[.1,1,.05,4,5]])
        s=grade_speeds(w)
        self.assertAlmostEqual(s[4],9)
        self.assertTrue(np.isnan(s[6]))

    def test_matched_grade_comparison_does_not_confuse_terrain_mix_with_slowdown(self):
        # Same pace within each grade, opposite distance mix between phases.
        w=np.array([[2,4,.01,0,4],[1,8,.05,4,12],[1,2,.01,60,62],[2,16,.05,62,78]])
        ref=np.array([1,1,0,0],bool)
        self.assertAlmostEqual(matched_ratio(w,ref,~ref)['ratio'],1)
        w[2:,1]*=1.5
        self.assertAlmostEqual(matched_ratio(w,ref,~ref)['ratio'],1.5)

    def test_aggregate_probe_uses_no_future_time(self):
        v=np.array([np.ones(100)*.1,np.ones(100),np.zeros(100),np.ones(100),np.zeros(100),np.zeros(100)])
        changed=v.copy();changed[1,61:]*=2
        a,b=early_forecasts(v),early_forecasts(changed)
        for x,y in zip(a,b):
            self.assertEqual(x['predicted_minutes'],y['predicted_minutes'])
            self.assertNotEqual(x['actual_minutes'],y['actual_minutes'])
        self.assertAlmostEqual(a[0]['predicted_minutes'],90)


if __name__=='__main__':unittest.main()
