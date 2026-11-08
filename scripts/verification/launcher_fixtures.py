#!/usr/bin/env python3
"""Run isolated launcher fixtures with cleanup-aware signal interruption."""

import argparse
import importlib.util
from pathlib import Path
import signal
import sys
import unittest


active_result = None


class Runner(unittest.TextTestRunner):
    def _makeResult(self):
        global active_result
        active_result = super()._makeResult()
        return active_result


def interrupt(signum, frame):
    if active_result is not None:
        active_result.stop()
    # unittest records an ordinary exception, runs the current test's cleanups,
    # and observes shouldStop before starting another test.
    raise InterruptedError(f"Launcher fixtures interrupted by signal {signum}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tests", type=Path, required=True)
    args = parser.parse_args()
    path = args.tests.resolve(strict=True)
    spec = importlib.util.spec_from_file_location("bilicomics_launcher_fixtures", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGHUP, interrupt)
    signal.signal(signal.SIGINT, interrupt)
    suite = unittest.defaultTestLoader.loadTestsFromModule(module)
    result = Runner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() and not result.shouldStop else 1


if __name__ == "__main__":
    sys.exit(main())
