# `ah ollama`

Dynamic plugin domain for Ollama Local API.

This domain is provided by external plugin `ah-plugin-ollama` and is loaded from `plugins` directory next to `ah`.

## Build and Install (Windows example)

```powershell
cargo build --release -p ah-plugin-ollama
New-Item -ItemType Directory -Force plugins | Out-Null
Copy-Item target/release/ah_plugin_ollama.dll plugins/ah-plugin-ollama.dll
```

Linux/macOS use `.so` / `.dylib`.

## `ah ollama ask`

Single prompt generation via `POST /api/generate` with non-streaming mode.

```bash
ah ollama ask --model <MODEL> --prompt <TEXT> [--system <TEXT>] [--base-url <URL>] [--timeout-secs <SECONDS>]
```

Examples:

```bash
ah ollama ask --model llama3.2 --prompt "Summarize this diff in 3 bullets"
ah --json ollama ask --model qwen2.5-coder --prompt "Generate test cases for parser"
```

## `ah ollama chat`

Single user message chat via `POST /api/chat` with non-streaming mode.

```bash
ah ollama chat --model <MODEL> --message <TEXT> [--system <TEXT>] [--base-url <URL>] [--timeout-secs <SECONDS>]
```

Examples:

```bash
ah ollama chat --model llama3.2 --message "Propose a refactor plan for this module"
ah --json ollama chat --model mistral --message "Write concise commit message"
```

## Notes

- Default base URL: `http://127.0.0.1:11434`
- Plugin returns plain text by default; with global `--json` returns structured payload.
- If Ollama is unavailable, command returns plugin error codes:
  - `OLLAMA_HTTP_FAILED`
  - `OLLAMA_API_FAILED`
  - `OLLAMA_RESPONSE_INVALID`
