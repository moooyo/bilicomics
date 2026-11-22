import unittest

from bench_admission import BOUNDARY, LOCK_LINE, ORDERS, block_summary, launcher_prefix


class LibraryFixtureTests(unittest.TestCase):
    def samples(self, deltas):
        return [{'order': order, 'B_minus_A_ns': delta} for order, delta in zip(ORDERS, deltas)]

    def test_per_block_gate_includes_the_one_millisecond_boundary(self):
        self.assertTrue(block_summary(self.samples([-1_000_000] * 5 + [2]))['passes_host_gate'])
        self.assertFalse(block_summary(self.samples([-999_999] * 6))['passes_host_gate'])
        self.assertFalse(block_summary(self.samples([-2_000_000] * 4 + [0, 1]))['passes_host_gate'])

    def test_missing_and_reordered_pairs_are_rejected(self):
        with self.assertRaises(ValueError):
            block_summary(self.samples([0] * 5))
        with self.assertRaises(ValueError):
            block_summary(self.samples([0] * 6)[::-1])

    def test_missing_or_duplicated_worker_boundary_is_rejected(self):
        for source in ('#!/bin/sh\n', '#!/bin/sh\n' + (BOUNDARY + LOCK_LINE) * 2):
            with self.assertRaisesRegex(ValueError, 'pre-lock boundary'):
                launcher_prefix(source)


if __name__ == '__main__':
    unittest.main()
