"""Every velo_core::Error variant has its own exception class (18.8-f)."""

import velo
from velo import _velo

# Documented renames: the class name differs from the core variant so it does
# not shadow a Python builtin or read ambiguously.
RENAMED = {"Io": "VeloIOError", "Db": "DatabaseError"}


def test_every_variant_has_its_own_exception_class():
    pairs = _velo._error_mapping()
    assert pairs, "no samples"
    for variant, cls in pairs:
        assert cls != "VeloError", f"{variant} falls through to the generic base"
        assert cls == RENAMED.get(variant, variant), (variant, cls)
        klass = getattr(velo, cls)
        assert issubclass(klass, velo.VeloError)


def test_variants_map_to_distinct_classes():
    classes = [cls for _, cls in _velo._error_mapping()]
    assert len(set(classes)) == len(classes)


def test_recent_variants_are_exported():
    for name in (
        "UnsafePath",
        "LimitExceeded",
        "AccessDenied",
        "WouldOverwriteEdits",
        "MissingCleanChanges",
        "SnapshotHasDependents",
        "RemoteRefAbsent",
        "RedoBranchMoved",
    ):
        assert name in velo.__all__
        assert issubclass(getattr(velo, name), velo.VeloError)
