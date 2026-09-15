import itertools
from pathlib import Path
import sys
import unittest

import numpy as np

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from enrichment_match import Network, decode_chain, interval_status


def way(identifier,nodes,coordinates):
    return dict(id=identifier,nodes=nodes,coordinates=(np.array(coordinates)/111195.08).tolist(),
                tags={"highway":"path"})


class MatchingContracts(unittest.TestCase):
    def test_dynamic_program_matches_exhaustive_sequence_costs(self):
        emissions = [np.array([1.,3.]),np.array([4.,0.]),np.array([0.,2.])]
        transitions = [np.array([[0.,6.],[3.,1.]]),np.array([[1.,0.],[5.,2.]])]
        choices = list(itertools.product(range(2),repeat=3))
        costs = [sum(emissions[i][p[i]] for i in range(3))+
                 sum(transitions[i][p[i],p[i+1]] for i in range(2)) for p in choices]
        path,totals = decode_chain(emissions,transitions)
        self.assertEqual(tuple(path),choices[int(np.argmin(costs))])
        for i in range(3):
            for candidate in range(2):
                self.assertEqual(totals[i][candidate],min(c for p,c in zip(choices,costs) if p[i]==candidate))

    def test_connected_sequence_resists_nearest_parallel_path(self):
        graph = Network([way(1,[1,2],[[0,0],[100,0]]),way(2,[3,4],[[0,10],[100,10]])],[0,0])
        xy = np.array([[5,0],[30,0],[60,7],[90,0]])
        track = np.c_[xy/111195.08,np.arange(4)*10]
        _,candidates,chosen,margin,connected = graph.match(track)
        self.assertEqual(candidates[2][0]["way"],1)
        self.assertTrue(all(candidates[i][chosen[i]]["way"]==0 for i in range(4)))
        self.assertTrue(connected.all())
        self.assertTrue((margin>0).all())

    def test_partial_edges_preserve_attribute_distance_at_junction(self):
        graph = Network([way(1,[1,2],[[0,0],[100,0]]),way(2,[2,3],[[100,0],[100,100]])],[0,0])
        a = graph.candidates(np.array([80,0]))[0]
        b = graph.candidates(np.array([100,30]))[0]
        length,pieces = graph.route(a,b,True)
        self.assertAlmostEqual(length,50)
        totals = {i:sum(d for w,d in pieces if w==i) for i in (0,1)}
        self.assertAlmostEqual(totals[0],20)
        self.assertAlmostEqual(totals[1],30)

    def test_disconnected_ways_do_not_create_a_surface_bridge(self):
        graph = Network([way(1,[1,2],[[0,0],[20,0]]),way(2,[3,4],[[200,0],[220,0]])],[0,0])
        track = np.array([[10/111195.08,0,0],[210/111195.08,0,20]])
        _,_,_,_,connected = graph.match(track)
        self.assertFalse(connected[0])

    def test_long_timestamp_gap_splits_even_an_easy_geometric_match(self):
        graph = Network([way(1,[1,2],[[0,0],[100,0]])],[0,0])
        track = np.array([[10/111195.08,0,0],[50/111195.08,0,60]])
        _,_,_,_,connected = graph.match(track)
        self.assertFalse(connected[0])

    def test_ambiguous_and_detouring_matches_are_not_accepted(self):
        candidate = dict(offset=2)
        args = (candidate,candidate,10,10,True,True,30,30)
        self.assertEqual(interval_status(*args),"accepted")
        self.assertEqual(interval_status(candidate,candidate,1,10,True,True,30,30),"ambiguous")
        self.assertEqual(interval_status(candidate,candidate,10,10,True,True,30,100),"length_disagreement")


if __name__ == "__main__":
    unittest.main()
