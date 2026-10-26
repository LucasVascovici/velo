"""Every velody_core::Error variant has its own exception class (18.8-f)."""

import velody
from velody import _velody

# Documented renames: the class name differs from the core variant so it does
# not shadow a Python builtin or read ambiguously.
RENAMED = {"Io": "VelodyIOError", "Db": "DatabaseError"}


def test_every_variant_has_its_own_exception_class():
    pairs = _velody._error_mapping()
    assert pairs, "no samples"
    for variant, cls in pairs:
        assert cls != "VelodyError", f"{variant} falls through to the generic base"
        assert cls == RENAMED.get(variant, variant), (variant, cls)
        klass = getattr(velody, cls)
        assert issubclass(klass, velody.VelodyError)


def test_variants_map_to_distinct_classes():
    classes = [cls for _, cls in _velody._error_mapping()]
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
        "NothingToUndo",
        "NothingToRedo",
        "RedoBranchMoved",
    ):
        assert name in velody.__all__
        assert issubclass(getattr(velody, name), velody.VelodyError)
