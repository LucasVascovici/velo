"""Threading: a handle belongs to one thread, and long calls release the GIL."""

import shutil
import threading
import time

import pytest
import velo

SNAPSHOTS = 3000


@pytest.fixture(scope="module")
def big(tmp_path_factory):
    """A repository of a few thousand snapshots, built with save_tree."""
    path = tmp_path_factory.mktemp("big")
    repo = velo.Repo.init(path)
    parent = None
    for i in range(SNAPSHOTS):
        parent = repo.save_tree(
            branch="main",
            message=f"s{i}",
            entries={"f.txt": f"line {i}\n".encode(), f"d/{i % 20}.txt": b"x"},
            parent=parent,
            timestamp_ms=1_000 + i,
        )
    return path, repo


class Counter:
    """A background Python thread that can only tick while it holds the GIL."""

    def __init__(self):
        self.ticks = []
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def _run(self):
        while not self._stop.is_set():
            self.ticks.append(time.perf_counter())

    def __enter__(self):
        self._thread.start()
        while not self.ticks:
            time.sleep(0.001)
        return self

    def __exit__(self, *exc):
        self._stop.set()
        self._thread.join()


# Longer than the 5 ms switch interval, so a tick this far from either edge of
# a call cannot be the interpreter handing over the GIL at the call boundary.
MARGIN = 0.010
MIN_CALL = 0.050


def ticks_inside(call):
    """Tick times strictly inside one `call`, away from its edges, and its length."""
    with Counter() as c:
        start = time.perf_counter()
        call()
        end = time.perf_counter()
        ticks = list(c.ticks)
    inside = [t for t in ticks if start + MARGIN <= t <= end - MARGIN]
    return inside, end - start


def test_fsck_releases_the_gil(big, tmp_path):
    path, _ = big
    # A later fsck of the same repository is far faster than the first, so check
    # a pristine copy that nothing has verified yet.
    copy = tmp_path / "copy"
    shutil.copytree(path, copy)
    repo = velo.Repo.open(copy)
    inside, took = ticks_inside(lambda: repo.fsck())
    assert took >= MIN_CALL, f"fsck took only {took:.3f}s, too short to discriminate"
    # Held for the whole call, ticks could only land at its edges.
    assert inside, f"fsck ran {took:.3f}s without letting another thread run"


def test_another_thread_gets_a_clear_error(big):
    _, repo = big
    seen = []

    def use():
        try:
            repo.history(branch="main", limit=1)
        except BaseException as e:  # noqa: BLE001 - the type is what is asserted
            seen.append(e)

    t = threading.Thread(target=use)
    t.start()
    t.join()
    assert len(seen) == 1
    assert type(seen[0]) is RuntimeError
    assert "only be used on the thread that created it" in str(seen[0])


def test_every_long_method_checks_the_owner(big):
    _, repo = big
    tip = repo.branch_tip("main")
    key = velo.SigningKey.from_seed(bytes(32))
    calls = [
        lambda: repo.save_tree(branch="t", message="m", entries={"a": b"a"}),
        lambda: repo.tree_at(tip),
        lambda: repo.history(branch="main"),
        lambda: repo.find_snapshots([["a", "b"]]),
        lambda: repo.blame("f.txt"),
        lambda: repo.merge_base(tip, tip),
        lambda: repo.retention("main", dry_run=True),
        lambda: repo.compact("main", [], dry_run=True),
        lambda: repo.fsck(),
        lambda: repo.sign(tip, key),
        lambda: repo.verify(tip, []),
        lambda: repo.signed_snapshots(),
        lambda: repo.sync("origin"),
    ]
    errors = []

    def use(call):
        try:
            call()
        except BaseException as e:  # noqa: BLE001
            errors.append(e)

    for call in calls:
        t = threading.Thread(target=use, args=(call,))
        t.start()
        t.join()
    assert len(errors) == len(calls)
    assert all(type(e) is RuntimeError and "unsendable" in str(e) for e in errors)


def test_one_handle_per_thread_runs_in_parallel(big):
    path, _ = big
    results = []

    def work():
        mine = velo.Repo.open(path)
        results.append(len(mine.history(branch="main")))

    threads = [threading.Thread(target=work) for _ in range(3)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert results == [SNAPSHOTS] * 3
