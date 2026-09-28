import unittest

from alternatives import (
    Route, dominates, experiment, linear_regret_2d, select, shared_length_fraction,
)


class AlternativeSelection(unittest.TestCase):
    def test_scalar_regret_loses_a_constraint_optimum(self):
        result = experiment()["unsupported_tradeoff"]
        self.assertTrue(result["all_nondominated"])
        self.assertEqual(result["linear_regret_over_all_weights"], 0)
        self.assertEqual(result["best_with_second_cost_at_most_6"], (6, 6))
        self.assertFalse(result["constraint_choice_retained"])

    def test_regret_maximum_can_be_inside_weight_interval(self):
        self.assertAlmostEqual(linear_regret_2d(((1, 4), (2, 2), (4, 1)),
                                              ((1, 4), (4, 1))), 0.2)

    def test_both_alternative_types_survive_without_forcing_count(self):
        result = experiment()["candidate_filter"]
        self.assertEqual(result["selected"], ["primary", "equal_quality_corridor",
                                             "valley", "smooth_surface"])
        self.assertEqual(result["rejected"], {"parallel_street": "same_corridor",
                                              "bad_detour": "excessive_cost"})
        self.assertEqual(result["parallel_street_edge_overlap"], 0)
        self.assertEqual(result["parallel_street_spatial_separation"], (0, 0))
        self.assertEqual(result["only_one_case"], ["primary"])

    def test_substantive_same_corridor_tradeoff_survives_cosmetic_street_does_not(self):
        result = experiment()["candidate_filter"]
        self.assertEqual(result["same_corridor_tradeoff_case"], ["primary", "same_corridor_smooth"])
        self.assertEqual(result["same_corridor_tradeoff_rejected"], {"parallel_street": "same_corridor"})

    def test_long_common_approach_does_not_hide_local_pass_choice(self):
        primary = Route("direct", (1_000_000, 500, 1000),
                        (("approach", 495_000), ("direct", 10_000), ("exit", 495_000)),
                        ((0, 0), (495_000, 0), (505_000, 0), (1_000_000, 0)))
        alternate = Route("other_pass", (1_000_770, 500, 1000),
                          (("approach", 495_000), ("other_pass", 10_770), ("exit", 495_000)),
                          ((0, 0), (495_000, 0), (500_000, 2000), (505_000, 0), (1_000_000, 0)))
        self.assertEqual(shared_length_fraction(primary, alternate), 0.99)
        selected, rejected = select(primary, [primary, alternate], (1, 2, 0.2))
        self.assertEqual(selected, [primary, alternate])
        self.assertEqual(rejected, {})

    def test_overlap_counts_length_not_segments_and_equal_vectors_are_not_dominated(self):
        a = Route("a", (1, 1), (("long", 900), ("short-a", 100)), ())
        b = Route("b", (1, 1), (("long", 900), ("short-b", 100)), ())
        self.assertEqual(shared_length_fraction(a, b), 0.9)
        self.assertFalse(dominates(a.costs, b.costs))
        self.assertTrue(experiment()["equal_vector_geographic_route_removed_without_regret"])


if __name__ == "__main__":
    unittest.main()
