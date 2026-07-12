# Recipe: Reuse Command Recipes

## Goal
Save repetitive command chains once and execute by short name.

## Commands
```bash
ah task save <name> <command>
ah task list
ah task run <name> [--timeout-secs 600] [--max-output-bytes 65536]
```

## Example
```bash
ah task save quick-diff "ah git changed --json && ah git diff --limit 120"
ah task list
ah task run quick-diff
```

## Output Shape
- `task list --json`: task catalog from `.ah/tasks.json`
- `task run --json`: exit status + captured stdout/stderr
- task output is byte-bounded while read; timeout terminates descendant processes

## When To Use
- Standardize repetitive AI support workflows
- Reduce command verbosity in prompts
- Keep project-local command conventions in one place
