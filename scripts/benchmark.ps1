param(
    [int]$Iterations = 3
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

Write-Host "Building release binary..."
cargo build --release | Out-Null

$binPath = Join-Path $root "target/release/ah.exe"
if (-not (Test-Path $binPath)) {
    throw "Release binary not found at $binPath"
}

$commands = @(
    @{
        Name = "file.read.range"
        Args = @("file", "read", "src/commands/ctx.rs", "-n", "--from", "1", "--to", "200")
    },
    @{
        Name = "search.text.rs"
        Args = @("search", "text", "execute", "src", "--glob", "*.rs", "--limit", "100")
    },
    @{
        Name = "ctx.symbols.review"
        Args = @("--json", "ctx", "symbols", "src/commands", "--preset", "review", "--limit", "80")
    },
    @{
        Name = "ctx.pack.summary"
        Args = @("--json", "ctx", "pack", "src", "--preset", "summary", "--limit", "120")
    },
    @{
        Name = "git.changed"
        Args = @("--json", "git", "changed")
    }
)

$results = @()

foreach ($commandSpec in $commands) {
    $times = @()
    for ($i = 1; $i -le $Iterations; $i++) {
        $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
        & $binPath @($commandSpec.Args) | Out-Null
        $stopwatch.Stop()
        $times += [math]::Round($stopwatch.Elapsed.TotalMilliseconds, 2)
    }

    $average = [math]::Round(($times | Measure-Object -Average).Average, 2)
    $minimum = [math]::Round(($times | Measure-Object -Minimum).Minimum, 2)
    $maximum = [math]::Round(($times | Measure-Object -Maximum).Maximum, 2)

    $results += [PSCustomObject]@{
        Command = $commandSpec.Name
        AvgMs = $average
        MinMs = $minimum
        MaxMs = $maximum
        Iterations = $Iterations
    }
}

$results | Format-Table -AutoSize

$benchDir = Join-Path $root "benchmarks"
New-Item -ItemType Directory -Force -Path $benchDir | Out-Null

$timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss K"
$markdownPath = Join-Path $benchDir "latest.md"

$markdown = @(
    "# AIHelper Benchmarks",
    "",
    "- Timestamp: $timestamp",
    "- Iterations per command: $Iterations",
    "",
    "| Command | Avg (ms) | Min (ms) | Max (ms) |",
    "|---|---:|---:|---:|"
)

foreach ($row in $results) {
    $markdown += "| $($row.Command) | $($row.AvgMs) | $($row.MinMs) | $($row.MaxMs) |"
}

$markdown -join "`n" | Set-Content -Path $markdownPath -Encoding UTF8

Write-Host ""
Write-Host "Benchmark report saved to $markdownPath"
