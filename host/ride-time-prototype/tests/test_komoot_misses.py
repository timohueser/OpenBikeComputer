"""Authored contracts for miss decomposition and prospective horizon probes."""

from pathlib import Path
import sys
import unittest

import numpy as np

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from komoot_misses import distance_weights,factors,time_weights


class MissTests(unittest.TestCase):
    def test_distance_horizon_is_complete_and_prorates_endpoint(self):
        d=np.array([1.,2.,3.])
        np.testing.assert_allclose(distance_weights(d,1,3),[0,1,1/3])
        self.assertIsNone(distance_weights(d,1,6))

    def test_recent_time_does_not_include_future_or_zero_time(self):
        t=np.array([10.,0.,20.,15.])
        np.testing.assert_allclose(time_weights(t,15,30),[0,0,.75,0])

    def test_early_grade_factors_and_fallback_do_not_use_future_times(self):
        d=np.ones(4);q=np.ones(4);t=np.array([2.,4.,8.,9.]);g=np.array([0,.05,0,.15])
        early=np.array([1,1,0,0],bool)
        scalar,grade,bins,support=factors(d,t,q,g,early)
        self.assertEqual(scalar,3)
        np.testing.assert_allclose(grade[bins],[2,4,2,3])
        t[~early]*=10
        b=factors(d,t,q,g,early)
        np.testing.assert_allclose(b[1],grade)

    def test_decomposition_separates_changed_grade_mix_from_changed_pace(self):
        d=np.array([2.,1.,1.,2.]);q=d.copy();t=d*np.array([2.,4.,2.,4.]);g=np.array([0,.05,0,.05])
        early=np.array([1,1,0,0],bool)
        scalar,grade,bins,_=factors(d,t,q,g,early)
        later=~early
        mix=((scalar-grade[bins])*q)[later].sum()
        within=(grade[bins]*q-t)[later].sum()
        self.assertAlmostEqual(within,0)
        self.assertAlmostEqual(mix,-2)
        self.assertAlmostEqual(mix+within,(scalar*q-t)[later].sum())


if __name__=='__main__':unittest.main()
