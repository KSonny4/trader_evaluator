#!/usr/bin/env python3
"""
TODO/FIXME guardrails:
- `list`: show all TODO/FIXME occurrences in selected repo paths.
- `check`: fail if any TODO/FIXME lacks a tracking ID.

Tracking IDs accepted:
- TODO(#123) / FIXME(#123)
- TODO(LIN-123) / FIXME(LIN-123)
- TODO(Task 38) / FIXME(Task-38)
"""

from __future__ import annotations

import argparse
import dataclasses
import os
import re
import sys
from typing import Iterable, List, Optional, Sequence, Tuple


# Only count TODO/FIXME when it appears in a comment context. This avoids false
# positives like string literals, docstrings, or regex source code.
_COMMENT_TODO_RE = re.compile(r"(?:^|\s)(//|#|/\*+|<!--)\s*(TODO|FIXME)\b")
_TRACKED_RE = re.compile(
    r"(?:^|\s)(//|#|/\*+|<!--)\s*(TODO|FIXME)\(\s*(#\d+|LIN-\d+|Task[- ]\d+)\s*\)",
    re.IGNORECASE,
)


@dataclasses.dataclass(frozen=True)
class TodoHit:
    path: str
    line_no: int
    text: str


@dataclasses.dataclass(frozen=True)
class ScanResult:
    all: List[TodoHit]
    untracked: List[TodoHit]


def _is_ignored_dir(name: str) -> bool:
    return name in {
        ".git",
        ".worktrees",
        "target",
        "archive",
        "__pycache__",
        ".venv",
        "node_modules",
    }


def _default_roots(root: str) -> List[str]:
    # Keep scope tight: code + scripts + deploy + hooks + CI.
    # Docs are intentionally excluded to reduce noise.
    return [
        os.path.join(root, "crates"),
        os.path.join(root, "scripts"),
        os.path.join(root, "deploy"),
        os.path.join(root, "hooks"),
        os.path.join(root, ".github"),
        os.path.join(root, "config"),
    ]


def _iter_files(roots: Sequence[str]) -> Iterable[str]:
    for r in roots:
        if not os.path.exists(r):
            continue
        if os.path.isfile(r):
            yield r
            continue

        for dirpath, dirnames, filenames in os.walk(r):
            dirnames[:] = [d for d in dirnames if not _is_ignored_dir(d)]
            for fn in filenames:
                yield os.path.join(dirpath, fn)


def _should_scan_file(path: str) -> bool:
    # Avoid scanning binaries; keep to typical repo text files.
    ext = os.path.splitext(path)[1].lower()
    return ext in {
        ".rs",
        ".sh",
        ".py",
        ".toml",
        ".yml",
        ".yaml",
        ".json",
    }


def _scan_file(path: str) -> Tuple[List[TodoHit], List[TodoHit]]:
    hits: List[TodoHit] = []
    untracked: List[TodoHit] = []

    if not _should_scan_file(path):
        return hits, untracked

    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            for i, line in enumerate(f, start=1):
                if not _COMMENT_TODO_RE.search(line):
                    continue
                text = line.rstrip("\n")
                hit = TodoHit(path=path, line_no=i, text=text)
                hits.append(hit)
                if not _TRACKED_RE.search(line):
                    untracked.append(hit)
    except OSError:
        # Non-fatal: treat unreadable files as skipped.
        return hits, untracked

    return hits, untracked


def scan_repo(root: str, roots: Optional[Sequence[str]] = None) -> ScanResult:
    roots = list(roots) if roots is not None else _default_roots(root)

    all_hits: List[TodoHit] = []
    untracked_hits: List[TodoHit] = []

    for p in _iter_files(roots):
        h, u = _scan_file(p)
        all_hits.extend(h)
        untracked_hits.extend(u)

    # Normalize paths for output stability.
    def rel(hit: TodoHit) -> TodoHit:
        return TodoHit(
            path=os.path.relpath(hit.path, root), line_no=hit.line_no, text=hit.text
        )

    all_hits = [rel(h) for h in all_hits]
    untracked_hits = [rel(h) for h in untracked_hits]
    return ScanResult(all=all_hits, untracked=untracked_hits)


def _print_hits(hits: Sequence[TodoHit]) -> None:
    for h in hits:
        print(f"{h.path}:{h.line_no}: {h.text}")


def main(argv: Sequence[str]) -> int:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)

    ap_list = sub.add_parser("list", help="List all TODO/FIXME occurrences")
    ap_list.add_argument("paths", nargs="*", help="Optional roots (default: repo code paths)")

    ap_check = sub.add_parser("check", help="Fail if any TODO/FIXME lacks a tracking ID")
    ap_check.add_argument("paths", nargs="*", help="Optional roots (default: repo code paths)")

    ns = ap.parse_args(list(argv))

    root = os.getcwd()
    roots = ns.paths if ns.paths else None
    res = scan_repo(root=root, roots=roots)

    if ns.cmd == "list":
        _print_hits(res.all)
        return 0

    if ns.cmd == "check":
        if res.untracked:
            print("Untracked TODO/FIXME found (add an ID: TODO(#123), TODO(LIN-123), TODO(Task 38)):")
            _print_hits(res.untracked)
            return 1
        return 0

    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
