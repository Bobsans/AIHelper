# Recipe: Read File With Line Numbers

## Goal
Read a file slice with stable line numbering for precise references.

## Command
```bash
ah file read <path> -n --from <start> --to <end>
```

## Example
```bash
ah file read src/main.rs -n --from 1 --to 80
```

## Output Shape
- Text mode: numbered lines (`"   1: ..."`)
- JSON mode (`--json`): object with `command`, `path`, `line_count`, `content`, and range flags

## When To Use
- Code review notes
- Refactoring with exact line references
- Small context extraction instead of full file dumps
