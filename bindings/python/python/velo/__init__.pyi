from __future__ import annotations

import datetime
import os
from typing import Any, Callable, Dict, List, Optional, Tuple, Union, final

__all__ = [
    "Author", "Blame", "BlameLine", "Branch", "Change", "Saved", "Entry", "LineOrigin",
    "MergePlan", "PlannedFile", "Repo", "Signature", "SigningKey", "SyncResult",
    "PushReport", "Verification", "Tag", "TreeEntry", "TreeFile", "VeloError",
    "NotARepo", "AlreadyInitialized", "NestedRepo", "SchemaTooNew",
    "MigrationRequired", "FormatTooOld", "Cancelled", "Locked",
    "DirtyWorkingTree", "OperationInProgress", "NoOperationInProgress",
    "Conflicts", "Diverged", "NotFastForward", "UnbornBranch", "NotFound",
    "AmbiguousPrefix", "Compacted", "Corrupt", "MissingObject",
    "UntrustedData", "UnsafePath", "LimitExceeded", "AccessDenied",
    "WouldOverwriteEdits", "MissingCleanChanges", "SnapshotHasDependents",
    "RemoteRefAbsent", "NothingToUndo", "NothingToRedo", "RedoBranchMoved", "InvalidInput", "Unsupported", "VeloIOError",
    "DatabaseError", "ReentrantCall",
]

class VeloError(Exception): ...
class NotARepo(VeloError):
    searched_from: str
class AlreadyInitialized(VeloError):
    at: str
class NestedRepo(VeloError):
    outer: str
class SchemaTooNew(VeloError):
    found: int
    supported: int
class MigrationRequired(VeloError):
    found: int
    supported: int
class FormatTooOld(VeloError):
    found: int
    supported: int
class Cancelled(VeloError): ...
class Locked(VeloError):
    held_by: Optional[int]
class DirtyWorkingTree(VeloError):
    paths: List[str]
class WouldOverwriteEdits(VeloError):
    paths: List[str]
class MissingCleanChanges(VeloError):
    path: str
    missing: int
    sample: List[str]
class SnapshotHasDependents(VeloError):
    snapshot: str
    dependents: int
    branches: List[Tuple[str, int]]
class OperationInProgress(VeloError):
    what: str
class NoOperationInProgress(VeloError):
    what: str
class Conflicts(VeloError):
    paths: List[str]
class Diverged(VeloError):
    branch: str
    ahead: int
    behind: int
class NotFastForward(VeloError):
    branch: str
    remote: str
class RemoteRefAbsent(VeloError):
    remote: str
    branch: str
    snapshot: str
class NothingToUndo(VeloError):
    branch: str
class NothingToRedo(VeloError):
    branch: str
class RedoBranchMoved(VeloError):
    branch: str
    snapshot: str
    tip: str | None
class UnbornBranch(VeloError):
    branch: str
class NotFound(VeloError):
    kind: str
    name: str
class AmbiguousPrefix(VeloError):
    prefix: str
    matches: int
class Compacted(VeloError):
    id: str
    into: str
class Corrupt(VeloError):
    detail: str
class MissingObject(VeloError):
    hash: str
class UntrustedData(VeloError):
    detail: str
class UnsafePath(VeloError):
    path: str
    problem: str
class LimitExceeded(VeloError):
    limit: str
    max: int
    found: Optional[int]
class AccessDenied(VeloError):
    remote: str
    detail: str
class InvalidInput(VeloError):
    detail: str
class Unsupported(VeloError):
    detail: str
class VeloIOError(VeloError): ...
class DatabaseError(VeloError): ...
class ReentrantCall(VeloError): ...

@final
class TreeEntry:
    path: str
    @staticmethod
    def file(path: str, data: bytes) -> TreeEntry: ...
    @staticmethod
    def executable(path: str, data: bytes) -> TreeEntry: ...
    @staticmethod
    def symlink(path: str, target: str) -> TreeEntry: ...
    @staticmethod
    def stored(path: str, object: str, kind: str = "regular") -> TreeEntry: ...

@final
class Change:
    @staticmethod
    def put(path: str, data: Union[bytes, str], kind: str = "regular") -> Change: ...
    @staticmethod
    def stored(path: str, object: str, kind: str = "regular") -> Change: ...
    @staticmethod
    def delete(path: str) -> Change: ...
    @staticmethod
    def rename(from_: str, to: str) -> Change: ...

@final
class Saved:
    snapshot: str
    written: Dict[str, str]

@final
class TreeFile:
    path: str
    object: str
    kind: str

@final
class Author:
    name: str
    email: Optional[str]
    def __new__(cls, name: str, email: Optional[str] = None) -> Author: ...

@final
class Entry:
    id: str
    message: str
    created_at: datetime.datetime
    created_at_ms: int
    branch: str
    parent: Optional[str]
    merge_parent: Optional[str]
    tag: Optional[str]
    is_merge: bool

@final
class LineOrigin:
    id: str
    created_at: datetime.datetime
    created_at_ms: int
    message: str
    author: Optional[Author]
    branch: str
    path: str

@final
class BlameLine:
    line_no: int
    text: str
    line_count: int
    origin: Optional[LineOrigin]

@final
class Blame:
    path: str
    snapshot: str
    lines: List[BlameLine]

@final
class Branch:
    name: str
    is_current: bool
    tip: Optional[str]
    tip_message: Optional[str]
    tip_created_at_ms: Optional[int]

@final
class PlannedFile:
    path: str
    action: str
    object: Optional[str]
    mode: Optional[int]
    content: Optional[bytes]
    base: Optional[str]
    ours: Optional[str]
    theirs: Optional[str]

@final
class MergePlan:
    base: Optional[str]
    files: List[PlannedFile]
    is_clean: bool
    conflicts: List[PlannedFile]

@final
class Tag:
    name: str
    snapshot: str
    message: Optional[str]

@final
class SigningKey:
    @staticmethod
    def from_seed(seed: bytes) -> SigningKey: ...
    @staticmethod
    def from_hex(text: str) -> SigningKey: ...
    @property
    def public_key(self) -> str: ...

@final
class Signature:
    snapshot: str
    algorithm: str
    public_key: str
    signature: str

@final
class Verification:
    snapshot: str
    trusted: List[str]
    untrusted: List[str]
    invalid: List[str]
    unverifiable: List[str]
    @property
    def is_trusted(self) -> bool: ...

@final
class PushReport:
    outcome: str
    branch: str
    remote: str
    snapshots: int
    objects: int
    created: Optional[str]
    also_published: List[str]

@final
class SyncResult:
    kind: str
    branch: str
    remote: str
    to: Optional[str]
    merge: Optional[str]
    paths: Optional[List[str]]
    report: Optional[PushReport]

@final
class Repo:
    @staticmethod
    def init(path: Union[str, os.PathLike]) -> Repo: ...
    @staticmethod
    def open(path: Union[str, os.PathLike]) -> Repo: ...
    def save_tree(
        self,
        *,
        branch: str,
        message: str,
        entries: Union[Dict[str, Union[bytes, str, TreeEntry]], List[TreeEntry]],
        parent: Optional[str] = None,
        merge_parent: Optional[str] = None,
        meta: Optional[Dict[str, Dict[str, str]]] = None,
        author: Optional[Author] = None,
        timestamp_ms: Optional[int] = None,
        renames: Optional[List[Tuple[str, str]]] = None,
    ) -> str: ...
    def save_changes(
        self,
        *,
        branch: str,
        message: str,
        changes: Union[Dict[str, Union[bytes, str, TreeEntry, None]], List[Change]],
        parent: Optional[str] = None,
        merge_parent: Optional[str] = None,
        meta: Optional[Dict[str, Dict[str, str]]] = None,
        author: Optional[Author] = None,
        timestamp_ms: Optional[int] = None,
    ) -> Saved: ...
    def tree_at(self, id: str) -> List[TreeFile]: ...
    def read_file_at(self, id: str, path: str) -> bytes: ...
    def read_files_at(self, id: str, paths: list[str]) -> list[tuple[str, bytes]]: ...
    def materialize(
        self,
        id: str,
        dest: str | os.PathLike[str],
        only: list[str] | None = None,
        ignore: list[str] | None = None,
    ) -> int: ...
    def read_object(self, object: str) -> bytes: ...
    def snapshot(self, id: str) -> Entry: ...
    def snapshot_meta(self, id: str) -> Dict[str, Dict[str, str]]: ...
    def resolve(self, spec: str) -> str: ...
    def is_trashed(self, id: str) -> bool: ...
    def branch_tip(self, branch: str) -> Optional[str]: ...
    def head_token(self) -> int: ...
    def history(
        self,
        *,
        from_: Optional[str] = None,
        branch: Optional[str] = None,
        all: bool = False,
        paths: Optional[List[str]] = None,
        limit: Optional[int] = None,
        meta: Optional[List[Tuple[str, ...]]] = None,
    ) -> List[Entry]: ...
    def find_snapshots(self, meta: List[Tuple[str, ...]]) -> List[Entry]: ...
    def blame(
        self,
        path: str,
        *,
        at: Optional[str] = None,
        lines: Optional[Tuple[int, int]] = None,
    ) -> Blame: ...
    def merge_base(self, a: str, b: str) -> Optional[str]: ...
    def merge_plan(self, ours: str, theirs: str) -> MergePlan: ...
    def merge_commit(
        self,
        *,
        branch: str,
        ours: str,
        theirs: str,
        message: str,
        resolutions: Optional[Dict[str, Union[str, bytes, None]]] = None,
        meta: Optional[Dict[str, Dict[str, str]]] = None,
        author: Optional[Author] = None,
        timestamp_ms: Optional[int] = None,
    ) -> str: ...
    def branches(self) -> List[Branch]: ...
    def create_branch(self, name: str, at: Optional[str] = None) -> None: ...
    def set_branch_tip(self, name: str, to: str) -> None: ...
    def on_event(
        self, callback: Callable[[Dict[str, Any]], Any]
    ) -> Callable[[Dict[str, Any]], Any]: ...
    def set_merge_driver(
        self,
        pattern: str,
        driver: Union[str, Callable[[str, str, str], Optional[str]]],
    ) -> None: ...
    def clear_merge_drivers(self) -> None: ...
    def retention(
        self,
        branch: str,
        tiers: Optional[List[Tuple[int, int]]] = None,
        now_ms: Optional[int] = None,
        dry_run: bool = False,
    ) -> Dict[str, Any]: ...
    def compact(
        self,
        branch: str,
        ranges: List[Tuple[str, str]],
        dry_run: bool = False,
    ) -> Dict[str, Any]: ...
    def tags(self) -> List[Tag]: ...
    def create_tag(
        self, name: str, at: Optional[str] = None, force: bool = False
    ) -> str: ...
    def delete_tag(self, name: str) -> None: ...
    def vacuum(self, full: bool = False) -> Dict[str, Any]: ...
    def repack(self, dry_run: bool = False) -> Dict[str, Any]: ...
    def fsck(self, repair: bool = False) -> Dict[str, List[Dict[str, str]]]: ...
    def sign(self, snapshot: str, key: SigningKey) -> Signature: ...
    def signatures(self, snapshot: str) -> List[Signature]: ...
    def verify(self, snapshot: str, trusted_keys: List[str]) -> Verification: ...
    def signed_snapshots(self) -> List[str]: ...
    def sync(
        self,
        remote: str = "origin",
        branch: Optional[str] = None,
        token: Optional[str] = None,
        author: Optional[Author] = None,
        message: Optional[str] = None,
        velo_bin: Optional[Union[str, os.PathLike]] = None,
    ) -> SyncResult: ...
