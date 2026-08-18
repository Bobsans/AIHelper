# Run retryable HTTP workflows with extracted variables

Use retries only for transient failures and keep them opt-in:

```bash
ah http get https://api.example.test/health \
  --retry 2 \
  --retry-delay-ms 250 \
  --expect-status 200
```

`--retry 2` means one initial attempt plus at most two additional attempts.
AIHelper retries transport failures, timeouts, response read failures, and HTTP
`5xx`. It does not retry `4xx` or failed assertions.

Be careful with POST, PUT, PATCH, and DELETE: a retry after an ambiguous network
failure or `5xx` can repeat a mutation that the remote service already applied.

For multi-step API checks, extract response values in one case and interpolate
them into later cases:

```yaml
version: 1
defaults:
  base_url: https://api.example.test
cases:
  - name: create session
    request:
      method: POST
      path: /sessions
    expect:
      status: 201
    extract:
      token:
        json: data.token
      request_id:
        header: X-Request-Id
      next_path:
        text:
          regex: '"next":"([^"]+)"'
          group: 1
  - name: use session
    request:
      path: '{{next_path}}'
      headers:
        authorization: 'Bearer {{token}}'
        x-request-id: '{{request_id}}'
    expect:
      status: 200
```

Run every case with the same retry policy:

```bash
ah http assert api.yaml --retry 2 --retry-delay-ms 250 --report json
```

Extraction is transactional. If an assertion or any extractor fails, the case
fails and none of its values become available to later cases. Reports contain
only extractor names and failure causes, never extracted values.
