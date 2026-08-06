# Task Scheduler readback normalization

## Problem

Windows Task Scheduler normalizes registration fields after
`RegisterTaskDefinition`: the registration URI becomes the task path, while
principal and logon-trigger user IDs may be returned as account names instead
of the SID supplied during registration. AIHelper currently treats those
equivalent values as foreign ownership and rejects a task it just created.

## Design

- Use the rooted task path as the canonical registration URI.
- Normalize principal and trigger account names to SID strings at the Windows
  Scheduler adapter boundary before constructing `ObservedTask`.
- Keep strict ownership checks for task path, registration source, and the
  validated JSON marker in task data.
- Keep semantic drift checks unchanged after normalization.

## Validation

- Unit-test the canonical URI and account-name-to-SID normalization helper.
- Run formatting, focused tests, workspace tests, and release build.
- Replace the installed `ah.exe`, reconcile the real task, and verify service
  readiness.
