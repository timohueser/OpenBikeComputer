"""The workers of a Python step of `obc data`: the cores that the step may use, and a process pool that ends with it."""

import os
import sys


def cores():
    """The cores that this process may use."""
    return len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else os.cpu_count()


def worker_count():
    """One worker for each core that this process can use, and one at most for each GB of free memory: a snow worker
    holds the planes of one chunk, about 0.6 GB for nine seasons of HR-WSI at 48° N (measured with two seasons)."""
    try:
        with open("/proc/meminfo") as meminfo:
            free = next(int(line.split()[1]) << 10 for line in meminfo if line.startswith("MemAvailable:"))
    except (OSError, StopIteration):
        return cores()
    return max(1, min(cores(), free >> 30))


def end_with(parent):
    """Make this worker end when the bake process `parent` ends, also when a signal kills it."""
    import ctypes
    import signal

    PR_SET_PDEATHSIG = 1
    ctypes.CDLL(None).prctl(PR_SET_PDEATHSIG, signal.SIGKILL)
    if os.getppid() != parent:
        os._exit(1)


def pool(workers=None):
    """`workers` processes, by default `worker_count()`, that fork from this process and end with it. A fork is not
    safe on macOS: there the pool runs `workers` threads, by default 4."""
    import multiprocessing
    from concurrent.futures import ProcessPoolExecutor, ThreadPoolExecutor

    if sys.platform == "linux":
        return ProcessPoolExecutor(workers or worker_count(), mp_context=multiprocessing.get_context("fork"),
                                   initializer=end_with, initargs=(os.getpid(),))
    return ThreadPoolExecutor(workers or 4)
