import datetime
import os
from typing import Dict, List, Optional, Tuple, Union

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
class InvalidInput(VeloError):
    detail: str
class Unsupported(VeloError):
    detail: str
class VeloIOError(VeloError): ...
class DatabaseError(VeloError): ...

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

class TreeFile:
    path: str
    object: str
    kind: str

class Author:
    name: str
    email: Optional[str]
    def __init__(self, name: str, email: Optional[str] = None) -> None: ...

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
    def tree_at(self, id: str) -> List[TreeFile]: ...
    def read_file_at(self, id: str, path: str) -> bytes: ...
    def read_object(self, object: str) -> bytes: ...
    def snapshot(self, id: str) -> Entry: ...
    def snapshot_meta(self, id: str) -> Dict[str, Dict[str, str]]: ...
    def resolve(self, spec: str) -> str: ...
    def branch_tip(self, branch: str) -> Optional[str]: ...
    def head_token(self) -> int: ...
