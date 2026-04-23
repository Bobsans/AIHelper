# Recipe: Inspect Directory Tree

## Goal
Get a compact project tree for navigation without dumping every file.

## Command
```bash
ah file tree <path> --depth <n> --limit <n>
```

## Example
```bash
ah file tree src --depth 2 --limit 80
```

## Output Shape
- Text mode: indented list (`root/`, nested `- item`)
- JSON mode (`--json`): flattened `entries[]` with `depth`, `kind`, `name`, `path`

## When To Use
- Fast orientation in unknown repositories
- Selecting target files before `ah file read`
- Producing compact context for AI planning
