"""Incomplete future stationary spans must not revise observations at forecast time."""

from pathlib import Path
import sys
import unittest
import numpy as np
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from komoot_motion_sensitivity import masks


class SensitivityTests(unittest.TestCase):
    def test_crossing_span_changes_only_future_outcome(self):
        data=dict(raw=np.array([np.ones(6),np.ones(6),np.zeros(6)]),elapsed_minutes=np.ones(6))
        spans=[dict(start_wall_min=0,end_wall_min=2),dict(start_wall_min=2,end_wall_min=4)]
        early,past,future=masks(data,spans,3)
        np.testing.assert_array_equal(early,[1,1,1,0,0,0])
        np.testing.assert_array_equal(past,[1,1,0,0,0,0])
        np.testing.assert_array_equal(future,[0,0,0,1,0,0])


if __name__=='__main__':unittest.main()
