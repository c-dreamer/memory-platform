<#
  Installs Memory Platform's scheduled jobs as Windows Task Scheduler tasks,
  mirroring launchd/ (macOS) and systemd/user/ (Linux) -- decision #22,
  docs/WINDOWS_PORT_SYNTHESIS.md. Templates live in taskscheduler/*.xml.template;
  this script substitutes __POWERSHELL__/__PS1_ARGS__/__USER__ and registers each
  one, same relationship as scripts/install-neon-sync-launchd.sh to launchd/.

  Existing bash runner scripts (run-memory-maintenance.sh, sync-to-neon.sh,
  verify-memory-archive.sh, start-memory-dashboard.sh, run-full-neon-recovery.sh)
  are reused as-is via Git Bash's bash.exe -- no PowerShell reimplementation of
  their logic, per decision #22's own scope (schedulers change, not the jobs).

  Usage:
    powershell -File scripts\install-taskscheduler.ps1
    powershell -File scripts\install-taskscheduler.ps1 -IncludeFullRecovery
#>
param(
  [switch]$IncludeFullRecovery
)
$ErrorActionPreference = 'Stop'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$StateHome = if ($env:XDG_STATE_HOME) { Join-Path $env:XDG_STATE_HOME 'memory-platform' } else { Join-Path $env:LOCALAPPDATA 'memory-platform' }
New-Item -ItemType Directory -Force -Path $StateHome | Out-Null

$bashCmd = $null
$onPath = Get-Command bash.exe -ErrorAction SilentlyContinue
if ($onPath) {
  $bashCmd = $onPath.Source
} else {
  foreach ($candidate in @("$env:ProgramFiles\Git\bin\bash.exe", "${env:ProgramFiles(x86)}\Git\bin\bash.exe")) {
    if (Test-Path $candidate) { $bashCmd = $candidate; break }
  }
}
if (-not $bashCmd) {
  throw "bash.exe not found. Git for Windows is required to run the existing shell-script job runners; install it or add bash.exe to PATH."
}
$pwshCmd = (Get-Command powershell.exe).Source
$userId = "$env:USERDOMAIN\$env:USERNAME"

# Placeholders below get spliced into XML text nodes via plain -replace, which
# has no XML awareness -- escape entities first so a path containing '&' (or
# a quote/angle bracket) doesn't produce malformed XML that Register-ScheduledTask rejects.
function ConvertTo-XmlText([string]$Value) {
  [System.Security.SecurityElement]::Escape($Value)
}

# MEMORY_LOCAL_ONLY is read from the same .env file the runner scripts source at
# run time, not just this shell's environment (decision #23). Uses the same
# ConvertFrom-EnvLine parser as mcp-transport-guard.ps1 (scripts/lib/env-file.ps1)
# so the two can't silently disagree on a quoted/commented value the way two
# independent hand-rolled parsers once did.
. (Join-Path $PSScriptRoot 'lib\env-file.ps1')
$EnvFile = if ($env:MEMORY_ENV_FILE) { $env:MEMORY_ENV_FILE } else { Join-Path $env:USERPROFILE '.config\memory-platform\memory.env' }
$localOnly = $false
if (Test-Path $EnvFile) {
  $raw = $null
  foreach ($line in Get-Content $EnvFile) {
    $parsed = ConvertFrom-EnvLine $line
    if ($parsed -and $parsed.Key -eq 'MEMORY_LOCAL_ONLY') { $raw = $parsed.Value }
  }
  if ($raw -and $raw.ToLowerInvariant() -match '^(1|true|yes)$') { $localOnly = $true }
}

# Neon = $true means "decision #23: skip when MEMORY_LOCAL_ONLY=1".
$jobs = @(
  @{ Name = 'MemoryPlatformNeonSync';       Runner = 'scripts\run-memory-maintenance.sh'; Args = @('run');          Neon = $true }
  @{ Name = 'MemoryPlatformNeonRetry';      Runner = 'scripts\run-memory-maintenance.sh'; Args = @('--retry-only'); Neon = $true }
  @{ Name = 'MemoryPlatformNeonCountAudit'; Runner = 'sync-to-neon.sh';                   Args = @('status');       Neon = $true }
  @{ Name = 'MemoryPlatformNeonReconcile';  Runner = 'sync-to-neon.sh';                   Args = @('reconcile');    Neon = $true }
  @{ Name = 'MemoryPlatformArchiveVerify';  Runner = 'scripts\verify-memory-archive.sh';  Args = @();               Neon = $false }
  @{ Name = 'MemoryPlatformDashboard';      Runner = 'scripts\start-memory-dashboard.sh'; Args = @();               Neon = $false }
)
if ($IncludeFullRecovery) {
  $jobs += @{ Name = 'MemoryPlatformFullRecovery'; Runner = 'scripts\run-full-neon-recovery.sh'; Args = @(); Neon = $true }
}

foreach ($job in $jobs) {
  if ($job.Neon -and $localOnly) {
    Write-Host "Skipping $($job.Name): MEMORY_LOCAL_ONLY is set (decision #23, docs/WINDOWS_PORT_SYNTHESIS.md)"
    Unregister-ScheduledTask -TaskName $job.Name -Confirm:$false -ErrorAction SilentlyContinue
    continue
  }

  $template = Join-Path $Root "taskscheduler\$($job.Name).xml.template"
  if (-not (Test-Path $template)) { throw "missing template: $template" }
  $runnerPath = Join-Path $Root $job.Runner
  if (-not (Test-Path $runnerPath)) { throw "missing runner script: $runnerPath" }

  $ps1Args = "-NoProfile -ExecutionPolicy Bypass -File `"$Root\scripts\run-with-rotation.ps1`" -LogName $($job.Name) -StateHome `"$StateHome`" -Bash `"$bashCmd`" -Runner `"$runnerPath`""
  if ($job.Args.Count -gt 0) {
    $ps1Args += ' -RunnerArgs ' + (($job.Args | ForEach-Object { "`"$_`"" }) -join ' ')
  }

  $xml = (Get-Content $template -Raw) `
    -replace '__POWERSHELL__', (ConvertTo-XmlText $pwshCmd) `
    -replace '__PS1_ARGS__', (ConvertTo-XmlText $ps1Args) `
    -replace '__USER__', (ConvertTo-XmlText $userId)

  Register-ScheduledTask -TaskName $job.Name -Xml $xml -Force | Out-Null
  Write-Host "Installed $($job.Name)"
}

Write-Host "Task Scheduler definitions installed. Memory Platform runs them per each task's own trigger."
