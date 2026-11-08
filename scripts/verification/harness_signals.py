"""Allow owned fixture processes to be cleaned up on stage interruption."""

import signal


def install_interruption_handlers():
    def interrupt(signum, frame):
        # The harnesses use finally blocks to stop their separately owned
        # application sessions before closing the X11 connection.
        raise KeyboardInterrupt(f"Fixture harness interrupted by signal {signum}")

    for signum in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(signum, interrupt)
