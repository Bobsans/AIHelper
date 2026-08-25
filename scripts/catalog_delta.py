#!/usr/bin/env python3
"""Report what a catalog snapshot change actually altered.

Deriving a schema from its Rust type reorders the `required` array, because the
derived list follows the property map rather than the declaration order. JSON
Schema treats `required` as a set, so that reordering is noise - but it hides the
real diffs in `git diff`.

This compares two snapshots with `required` canonicalised, so anything it prints
is a genuine change to the published contract and needs a decision.

Usage:
    # before touching a schema
    cp tests/snapshots/typed-command-catalog.snap target/catalog_prev.snap

    # after regenerating with AH_UPDATE_SNAPSHOTS=1
    python scripts/catalog_delta.py

    # or explicitly
    python scripts/catalog_delta.py <before.snap> <after.snap>

Exit code is 0 when the only difference is `required` ordering.
"""

from __future__ import annotations

import difflib
import json
import pathlib
import sys

DEFAULT_BEFORE = pathlib.Path("target/catalog_prev.snap")
DEFAULT_AFTER = pathlib.Path("tests/snapshots/typed-command-catalog.snap")


def canonical(node):
    """Sort every `required` array so ordering cannot mask a real difference."""
    if isinstance(node, dict):
        return {
            key: sorted(value)
            if key == "required" and isinstance(value, list)
            else canonical(value)
            for key, value in node.items()
        }
    if isinstance(node, list):
        return [canonical(value) for value in node]
    return node


def load(path: pathlib.Path):
    if not path.is_file():
        sys.exit(f"missing snapshot: {path}")
    return canonical(json.loads(path.read_text(encoding="utf-8")))


def describe(before, after) -> int:
    by_id = {entry["descriptor"]["id"]: entry for entry in before}
    after_by_id = {entry["descriptor"]["id"]: entry for entry in after}

    removed = sorted(set(by_id) - set(after_by_id))
    added = sorted(set(after_by_id) - set(by_id))
    for command in removed:
        print(f"-- removed command: {command}")
    for command in added:
        print(f"-- added command: {command}")

    changed = 0
    for command in sorted(set(by_id) & set(after_by_id)):
        old, new = by_id[command], after_by_id[command]
        if old == new:
            continue
        changed += 1
        old_lines = json.dumps(old, indent=1, sort_keys=True).splitlines()
        new_lines = json.dumps(new, indent=1, sort_keys=True).splitlines()
        print(f"-- {command}")
        print(
            "\n".join(
                difflib.unified_diff(
                    old_lines, new_lines, "before", "after", lineterm="", n=2
                )
            )
        )

    if not (removed or added or changed):
        print("OK: identical once `required` is treated as a set")
        return 0
    return 1


def main() -> int:
    arguments = sys.argv[1:]
    if len(arguments) == 2:
        before_path, after_path = (pathlib.Path(value) for value in arguments)
    elif not arguments:
        before_path, after_path = DEFAULT_BEFORE, DEFAULT_AFTER
    else:
        sys.exit(__doc__)
    return describe(load(before_path), load(after_path))


if __name__ == "__main__":
    raise SystemExit(main())
