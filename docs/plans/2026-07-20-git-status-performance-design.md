# Git Status Performance Design

## Goal

Preserve the complete `git status` text and JSON contracts while reducing the
number of child Git processes and their Windows startup cost.

## Execution

- Run `git status --porcelain=v2 --branch -z` once for repository detection,
  changed entries, branch, upstream, and ahead/behind counts.
- Run `git log -1 --format=%H%x00%s` and
  `git describe --tags --abbrev=0` concurrently after a successful status
  snapshot.
- Skip the separate `git --version` preflight for the built-in Git plugin.
  A missing executable is mapped at the Git I/O boundary to the existing
  `DEPENDENCY_MISSING` diagnostic.

## Compatibility

- Keep all released output fields and status-count semantics unchanged.
- Retain the porcelain v1 parser for `git changed` and `ctx changed`.
- Parse porcelain v2 as bytes with NUL boundaries so spaces, Unicode, arrows,
  and newlines in paths remain unambiguous.

## Validation

- Unit tests cover headers, ordinary changes, untracked files, renames,
  conflicts, unusual paths, and malformed records.
- Existing Git integration tests continue to validate output contracts.
- The release benchmark compares warm p50/p95 latency with event logging
  disabled, targeting 100-150 ms on the current Windows machine.

### Benchmark result

On the current Windows machine, 40 timed release runs after 5 warmups against
the same dirty repository, with event-log writes disabled, produced:

| Revision | p50 | p95 |
| --- | ---: | ---: |
| `HEAD` baseline | 381.60 ms | 409.70 ms |
| optimized worktree | 158.35 ms | 177.91 ms |

This reduces p50 by 58.5% and p95 by 56.6%. The optimized p50 remains 8.35 ms
above the upper edge of the absolute target on this repository.
