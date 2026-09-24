<#
Records a profiled play session of cw-client for frame-time analysis (Windows; see
profile.sh for macOS and Linux).

  .\profile.ps1                       build, play, then summarise
  .\profile.ps1 -Samply               also record a CPU sampling profile (needs `cargo install samply`;
                                      samply on Windows must run from an elevated prompt)
  .\profile.ps1 -DebugOverlay         build with the debug overlay
  .\profile.ps1 -ClientArgs '--quit-after','60'   arguments for cw-client
  .\profile.ps1 -NoteMs 2             log worker operations over 2 ms (default 4)

Output goes to profiles\<timestamp>\:
  trace.csv    one row per frame: wall time, update dt, every phase and lock wait (ms)
  stats.log    5 s summaries, slow frames, worker operations over NoteMs
  meta.txt     commit, OS, CPU, GPU and driver, display options
  cpu.json.gz  (with -Samply) open at https://profiler.firefox.com

Quit through the game (menu or closing the window), not Ctrl-C, so the trace is flushed.
If scripts are blocked: powershell -ExecutionPolicy Bypass -File .\profile.ps1
#>
param(
    [switch]$Samply,
    [switch]$DebugOverlay,
    [string[]]$ClientArgs = @(),
    [int]$NoteMs = 4
)
$ErrorActionPreference = 'Stop'
Set-Location -Path $PSScriptRoot

if ($Samply -and -not (Get-Command samply -ErrorAction SilentlyContinue)) {
    Write-Error 'samply not found; install it with: cargo install samply'
}

$gameDir = if ($env:CW_GAME_DIR) { $env:CW_GAME_DIR } else { Join-Path $PWD 'game' }
if (-not (Test-Path $gameDir -PathType Container)) {
    Write-Error "game folder not found at $gameDir (set CW_GAME_DIR)"
}

$buildArgs = @('build', '-p', 'cw-client', '--release')
if ($DebugOverlay) { $buildArgs += @('--features', 'debug-overlay') }
Write-Host "== building cw-client ($($buildArgs[3..($buildArgs.Count - 1)] -join ' '))"
& cargo @buildArgs
if ($LASTEXITCODE -ne 0) { Write-Error "cargo build failed ($LASTEXITCODE)" }

$out = Join-Path 'profiles' (Get-Date -Format 'yyyyMMdd-HHmmss')
New-Item -ItemType Directory -Force -Path $out | Out-Null
$out = (Resolve-Path $out).Path

# The machine: hitching on Windows often depends on the GPU driver and the timer.
$meta = @()
$meta += "date: $(Get-Date)"
$commit = 'unknown'
try {
    $c = (& git rev-parse --short HEAD 2>$null)
    if ($c) { $commit = $c }
    & git diff --quiet 2>$null
    if ($LASTEXITCODE -ne 0) { $commit += ' (uncommitted changes)' }
} catch { }
$meta += "commit: $commit"
$meta += "os: $([Environment]::OSVersion.VersionString)"
try {
    $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
    $meta += "cpu: $($cpu.Name.Trim()) ($($cpu.NumberOfCores) cores, $($cpu.NumberOfLogicalProcessors) threads)"
    foreach ($g in Get-CimInstance Win32_VideoController) {
        $meta += "gpu: $($g.Name), driver $($g.DriverVersion), $($g.CurrentHorizontalResolution)x$($g.CurrentVerticalResolution) @ $($g.CurrentRefreshRate) Hz"
    }
    $mem = Get-CimInstance Win32_ComputerSystem
    $meta += "memory: $([math]::Round($mem.TotalPhysicalMemory / 1GB, 1)) GB"
    $plan = (& powercfg /getactivescheme 2>$null)
    if ($plan) { $meta += "power plan: $plan" }
} catch {
    $meta += "machine info unavailable: $_"
}
$meta += "features: $(if ($DebugOverlay) { 'debug-overlay' } else { 'none' })"
$meta += "client args: $(if ($ClientArgs.Count) { $ClientArgs -join ' ' } else { 'none' })"
$opts = Join-Path $gameDir 'options.cfg'
if (Test-Path $opts) {
    $meta += 'options.cfg:'
    $meta += (Get-Content $opts | ForEach-Object { "  $_" })
}
$meta | Set-Content -Path (Join-Path $out 'meta.txt') -Encoding UTF8

$env:CW_GAME_DIR = $gameDir
$env:CW_CLIENT_TRACE = Join-Path $out 'trace.csv'
$env:CW_CLIENT_STATS = '1'
$env:CW_CLIENT_STATS_NOTE_MS = "$NoteMs"

$exe = Join-Path $PWD 'target\release\cw-client.exe'
if ($Samply) {
    $file = 'samply'
    $argv = @('record', '--save-only', '-o', (Join-Path $out 'cpu.json.gz'), $exe) + $ClientArgs
} else {
    $file = $exe
    $argv = $ClientArgs
}

Write-Host "== recording to $out (play, then quit through the game)"
$log = Join-Path $out 'stats.log'
$startArgs = @{ FilePath = $file; NoNewWindow = $true; Wait = $true; PassThru = $true; RedirectStandardError = $log }
# Windows PowerShell 5.1 joins ArgumentList with spaces without quoting.
if ($argv.Count) { $startArgs.ArgumentList = @($argv | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) }
$proc = Start-Process @startArgs
$status = $proc.ExitCode

Write-Host ''
Write-Host '== summary'
$trace = Join-Path $out 'trace.csv'
if ((Test-Path $trace) -and (Get-Item $trace).Length -gt 0) {
    $rows = @(Import-Csv $trace)
    # The first frames load the world and build the GUI; leave them out of the statistics.
    if ($rows.Count -gt 120) { $warm = $rows[60..($rows.Count - 1)] } else { $warm = $rows }
    if ($warm.Count -eq 0) {
        Write-Host 'no frames recorded'
    } else {
        $inv = [Globalization.CultureInfo]::InvariantCulture
        $num = { param($v) [double]::Parse($v, $inv) }
        $ft = @($warm | ForEach-Object { & $num $_.frame_ms } | Sort-Object)
        $pct = { param($p) $ft[[int][math]::Floor(($ft.Count - 1) * $p)] }
        $secs = (& $num $warm[-1].t_s) - (& $num $warm[0].t_s)
        $mean = ($ft | Measure-Object -Average).Average
        Write-Host ("{0} frames over {1:N0} s (first {2} skipped as warm-up)" -f $ft.Count, $secs, ($rows.Count - $warm.Count))
        Write-Host ("frame ms: mean {0:N2}, p50 {1:N2}, p90 {2:N2}, p99 {3:N2}, max {4:N1}" -f $mean, (& $pct 0.5), (& $pct 0.9), (& $pct 0.99), $ft[-1])
        foreach ($t in 20, 33, 50, 100) {
            Write-Host ("  over {0,3} ms: {1}" -f $t, @($ft | Where-Object { $_ -gt $t }).Count)
        }
        $phases = @($rows[0].PSObject.Properties.Name | Where-Object { $_ -notin 't_s', 'frame_ms', 'dt_ms' })
        Write-Host 'slowest frames (top phases, ms):'
        $slow = $warm | Sort-Object -Property @{ Expression = { & $num $_.frame_ms }; Descending = $true } | Select-Object -First 10
        foreach ($r in $slow) {
            $top = $phases | Sort-Object -Property @{ Expression = { & $num $r.$_ }; Descending = $true } | Select-Object -First 4
            $parts = ($top | Where-Object { (& $num $r.$_) -ge 0.5 } | ForEach-Object { '{0} {1:N1}' -f $_, (& $num $r.$_) }) -join ', '
            Write-Host ("  t={0,8:N2} s  {1,6:N1} ms  dt {2,3}  {3}" -f (& $num $r.t_s), (& $num $r.frame_ms), $r.dt_ms, $parts)
        }
    }
} else {
    Write-Host '(no trace recorded)'
}

Write-Host ''
Write-Host "== files in ${out}:"
Get-ChildItem $out | Format-Table Name, Length -AutoSize | Out-String | Write-Host
if ($status -ne 0) { Write-Host "cw-client exited with status $status" }
