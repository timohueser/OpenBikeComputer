import itertools
from pathlib import Path
import sys
import unittest

import numpy as np

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from matching_v2 import PathNetwork,common_composition,same_core,solve_chain,trim


def way(identifier,nodes,coordinates,surface="asphalt"):
    return dict(id=identifier,nodes=nodes,coordinates=(np.array(coordinates)/111195.08).tolist(),
                tags={"highway":"path","surface":surface})


class PathAgreementContracts(unittest.TestCase):
    def test_pair_margins_match_exhaustive_complete_sequences(self):
        emissions=[np.array([1.,3.]),np.array([4.,0.]),np.array([0.,2.])]
        transitions=[np.array([[0.,6.],[3.,1.]]),np.array([[1.,0.],[5.,2.]])]
        choices=list(itertools.product(range(2),repeat=3))
        costs=[sum(emissions[i][p[i]] for i in range(3))+sum(transitions[i][p[i],p[i+1]] for i in range(2)) for p in choices]
        path,margins=solve_chain(emissions,transitions)
        self.assertEqual(tuple(path),choices[int(np.argmin(costs))])
        for i in range(2):
            for a,b in itertools.product(range(2),repeat=2):
                expected=min(c for p,c in zip(choices,costs) if p[i:i+2]==(a,b))-min(costs)
                self.assertEqual(margins[i][a,b],expected)

    def test_junction_node_is_one_state_but_unconnected_crossing_is_not(self):
        graph=PathNetwork([way(1,[1,2],[[-20,0],[0,0]]),way(2,[2,3],[[0,0],[20,0]]),
                           way(3,[4,5],[[0,-20],[0,20]])],[0,0])
        candidates=graph.candidates(np.array([0.,0.]))
        self.assertEqual(len(candidates),2)
        self.assertEqual(sum(c['key']==('node',2) for c in candidates),1)

    def test_endpoint_shift_is_allowed_but_parallel_path_is_not(self):
        self.assertTrue(same_core([(1,0,100)],[(1,5,105)]))
        self.assertFalse(same_core([(1,0,100)],[(2,0,100)]))
        self.assertFalse(same_core([(1,0,10)],[(1,6,16)]))

    def test_longitudinal_uncertainty_on_one_unbranched_road(self):
        graph=PathNetwork([way(1,[1,2,3],[[0,0],[20,0],[40,0]]),
                           way(2,[3,4],[[40,0],[60,0]])],[0,0])
        self.assertTrue(graph.same_path([(0,0,10)],[(2,10,20)]))

    def test_branch_and_closed_ring_are_not_one_corridor(self):
        graph=PathNetwork([way(1,[1,2],[[0,0],[20,0]]),way(2,[2,3],[[20,0],[40,0]]),
                           way(3,[2,4],[[20,0],[20,20]])],[0,0])
        self.assertFalse(graph.same_path([(0,0,20)],[(2,0,20)]))
        ring=PathNetwork([way(1,[1,2,3,1],[[0,0],[20,0],[20,20],[0,0]])],[0,0])
        self.assertEqual(len(set(ring.corridors)),3)

    def test_internal_diversion_is_not_hidden_by_equal_endpoints(self):
        a=[(1,0,40),(2,0,5),(3,0,40)]
        b=[(1,0,40),(4,0,5),(3,0,40)]
        self.assertFalse(same_core(a,b))

    def test_trimming_preserves_direction_across_edges(self):
        self.assertEqual(trim([(1,0,5),(2,20,0),(3,0,5)],10),[(2,15,5)])
        self.assertEqual(trim([(1,0,5)],10),[])

    def test_unknown_and_conflicting_attributes_cannot_become_known(self):
        result=common_composition([{'asphalt':.7,'gravel':.3},{'asphalt':.5,'gravel':.5}])
        self.assertEqual(result,{'asphalt':.5,'gravel':.3})
        self.assertEqual(common_composition([{'asphalt':1},{'unknown':1}]),{})
        self.assertEqual(common_composition([{'asphalt':1},{'gravel':1}]),{})

    def test_attribute_composition_includes_the_intervening_path(self):
        graph=PathNetwork([way(1,[1,2],[[0,0],[20,0]]),way(2,[2,3],[[20,0],[80,0]],'gravel'),
                           way(3,[3,4],[[80,0],[100,0]])],[0,0])
        a=graph.candidates(np.array([10.,0.]))[0]
        b=graph.candidates(np.array([90.,0.]))[0]
        length,pieces=graph.route_pieces(a,b)
        self.assertAlmostEqual(length,80)
        mix=graph.composition(pieces,'surface')
        self.assertAlmostEqual(mix['gravel'],.75)
        self.assertAlmostEqual(mix['asphalt'],.25)


if __name__=='__main__':
    unittest.main()
