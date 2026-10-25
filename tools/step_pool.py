"""The workers of a Python step of `obc data`: the cores that the step may use, and a process pool that ends with it."""

import os
import sys


def cores():
    """The cores that this process may use."""
    return len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else os.cpu_count()


def worker_count(memory=1 << 30):
    """One worker for each core that this process can use, and one at most for each `memory` bytes of free memory
    past a quarter of the machine's memory: other work shares the machine, and its memory grows while the step runs.
    The default is a GB: a snow worker holds the planes of one chunk, about 0.6 GB for nine seasons of HR-WSI at 48° N
    (measured with two seasons)."""
    try:
        with open("/proc/meminfo") as meminfo:
            sizes = {line.split(":")[0]: int(line.split()[1]) << 10 for line in meminfo}
        free = sizes["MemAvailable"] - sizes["MemTotal"] // 4
    except (OSError, KeyError):
        return cores()
    # The memory that the other steps of the run leave free: `obc data` reserves only the largest process of a step.
    free = min(free, int(os.environ.get("OBC_DATA_MEMORY_BYTES", free)))
    return max(1, min(cores(), free // memory))


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


_shared = None


def _call(function, item):
    return function(_shared, item)


def forked(function, shared, items, workers=None):
    """function(shared, item) for each of `items` in a `pool` of `workers`, as each one ends. The workers fork
    once `shared` is set, so they read it without a copy."""
    from concurrent.futures import as_completed

    global _shared
    _shared = shared
    executor = pool(workers)
    try:
        for done in as_completed([executor.submit(_call, function, item) for item in items]):
            yield done.result()
    finally:
        executor.shutdown(cancel_futures=True)
        _shared = None
