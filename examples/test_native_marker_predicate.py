"""Synthetic predicate regressions; not native drawing/erasure proof."""
import copy
import math
import unittest
from native_marker_predicate import native, validate_lines, follows, TOLERANCE


def paths():
    result = []
    for width, top, bottom in [(15, 9, 38), (10, 16, 35), (5, 23, 32)]:
        a, b, c = (722, 934+top), (722+width, 934+bottom), (722-width, 934+bottom)
        result.extend([[a, b], [b, c], [c, a]])
    sa, sb, sc = math.dist(b, c), math.dist(a, c), math.dist(a, b)
    perimeter = sa+sb+sc
    cx, cy = [(sa*a[i]+sb*b[i]+sc*c[i])/perimeter for i in range(2)]
    radius = abs((b[0]-c[0])*(b[1]-a[1]))/perimeter
    result.append([(round(cx+radius*math.cos(i*math.tau/24)), round(cy+radius*math.sin(i*math.tau/24))) for i in range(25)])
    return result


def line(path):
    return dict(tool=17, color=0, color_rgba=None, move_id=None, starting_length=0.0,
                thickness_scale=2.748575448989868,
                points=[dict(x=native(p)[0], y=native(p)[1], width=22, pressure=180,
                             direction=0, speed=0) for p in path])


class MarkerPredicate(unittest.TestCase):
    def setUp(self):
        self.paths = paths()
        self.before = {'original': line([(100, 100), (200, 200)])}
        self.after = self.before | {f'new{i}': line(p) for i, p in enumerate(self.paths)}

    def test_ten_ordered_paths_positive_synthetic_control(self):
        self.assertEqual(len(validate_lines(self.before, self.after, self.paths)), 10)

    def test_metadata_only_or_partial_persistence_is_not_marker_proof(self):
        for after in [self.before, self.after | {'unexpected': line(self.paths[0])},
                      {k: v for k, v in self.after.items() if k != 'new9'}]:
            with self.assertRaises(ValueError): validate_lines(self.before, after, self.paths)

    def test_original_mutation_removal_and_unexpected_style_fail(self):
        for key, field, value in [('original', 'color', 1), ('new0', 'color', 1),
                                  ('new0', 'tool', 2), ('new0', 'thickness_scale', 4)]:
            after = copy.deepcopy(self.after); after[key][field] = value
            with self.assertRaises(ValueError): validate_lines(self.before, after, self.paths)
        after = dict(self.after); del after['original']
        with self.assertRaises(ValueError): validate_lines(self.before, after, self.paths)

    def test_same_endpoints_with_detour_reversed_path_and_wrong_duplicate_fail(self):
        for mutation in ['detour', 'reverse', 'duplicate']:
            after = copy.deepcopy(self.after)
            if mutation == 'detour':
                middle = dict(after['new0']['points'][0]); middle['x'] += 15
                after['new0']['points'].insert(1, middle)
            elif mutation == 'reverse': after['new0']['points'].reverse()
            else: after['new0'] = copy.deepcopy(after['new1'])
            with self.assertRaises(ValueError): validate_lines(self.before, after, self.paths)

    def test_fixed_transform_matches_independent_native_endpoints(self):
        # Actual previously retained top sentinel + selection failure native points.
        pairs = [((708,918),(813.8353271484375,2267.80322265625)),
                 ((737,918),(887.3148193359375,2267.80322265625)),
                 ((708,944),(813.8353271484375,2333.202880859375)),
                 ((737,973),(887.0980224609375,2406.34423828125)),
                 ((737,944),(886.716064453125,2333.202880859375)),
                 ((708,973),(813.4522094726562,2406.34521484375)),
                 ((710,946),(818.8657836914062,2338.110595703125)),
                 ((735,946),(882.2907104492188,2338.111572265625))]
        self.assertLess(max(math.dist(native(p), observed) for p, observed in pairs), .681)
        self.assertEqual(TOLERANCE, 1.0)

    def test_quantization_bound_and_complexity_limits(self):
        expected = [native(p) for p in self.paths[0]]
        self.assertFalse(follows([(x+2, y) for x, y in expected], expected))
        self.assertFalse(follows(expected*100, expected))


if __name__ == '__main__': unittest.main()
