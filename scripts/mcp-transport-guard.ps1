<#
  Windows port of scripts/mcp-transport-guard.sh (item 10, decision #20,
  docs/WINDOWS_PORT_SYNTHESIS.md §4 "Windows launcher"). Stdio transports
  belong to the client session that created them; this guard makes every
  launch observable and prunes only registry records for dead children --
  it never kills a live Codex/OpenCode/Claude Code MCP process.

  Per-agent client config (Claude Code .mcp.json, Codex ~/.codex/config.toml,
  etc.) points its "command" at THIS script instead of mcp-server.exe directly,
  same relationship mcp-entrypoint.sh has to mcp-transport-guard.sh on
  macOS/Linux.
#>
param(
  [Parameter(ValueFromRemainingArguments)]
  [string[]]$ServerArgs = @()
)
$ErrorActionPreference = 'Stop'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$EnvFile = if ($env:MEMORY_ENV_FILE) { $env:MEMORY_ENV_FILE } else { Join-Path $env:USERPROFILE '.config\memory-platform\memory.env' }
$ReleaseFile = Join-Path $Root 'target\release\.memory-platform-release'
$ServerExe = Join-Path $Root 'target\release\mcp-server.exe'
$StateDir = if ($env:XDG_STATE_HOME) { Join-Path $env:XDG_STATE_HOME 'memory-platform' } else { Join-Path $env:LOCALAPPDATA 'memory-platform' }
$RegistryDir = Join-Path $StateDir 'mcp-transports'
$LogFile = Join-Path $StateDir 'mcp-transport.log'

function Write-Log([string]$Line) {
  $stamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
  Add-Content -Path $LogFile -Value "$stamp $Line"
}

New-Item -ItemType Directory -Force -Path $RegistryDir | Out-Null
# icacls equivalent of chmod 700: restrict the registry dir to the current user.
# icacls's own exit code, not $ErrorActionPreference, is what actually reflects
# success -- it writes failures to its own streams rather than throwing.
icacls $RegistryDir /inheritance:r /grant:r "$($env:USERNAME):(OI)(CI)F" | Out-Null
if ($LASTEXITCODE -ne 0) {
  Write-Log "acl_grant_failed target=$RegistryDir exit=$LASTEXITCODE"
}

Get-ChildItem -Path $RegistryDir -Filter '*.pid' -ErrorAction SilentlyContinue | ForEach-Object {
  # A non-numeric BaseName is just another dead record to prune, matching the
  # bash original's `kill -0` (fails harmlessly on non-numeric input) -- an
  # unguarded [int] cast would throw and abort the guard before the MCP
  # server ever launches, instead of pruning and continuing.
  $recordPid = 0
  if (-not [int]::TryParse($_.BaseName, [ref]$recordPid) -or -not (Get-Process -Id $recordPid -ErrorAction SilentlyContinue)) {
    Remove-Item -Force $_.FullName -ErrorAction SilentlyContinue
  }
}

if (-not (Test-Path $EnvFile)) {
  Write-Log 'startup_failed reason=missing_environment'
  [Console]::Error.WriteLine('memory MCP environment file is missing')
  exit 78
}
if (-not (Test-Path $ServerExe) -or -not (Test-Path $ReleaseFile)) {
  Write-Log 'startup_failed reason=missing_release'
  [Console]::Error.WriteLine('memory MCP release is not installed')
  exit 78
}

# Plain KEY=VALUE lines only (the protected env file is hand-authored, not a
# shell script) -- optionally single/double-quoted, '#' comments, blank lines
# skipped. Sets vars for THIS process so they're inherited by the child below.
. (Join-Path $PSScriptRoot 'lib\env-file.ps1')
Get-Content $EnvFile | ForEach-Object {
  $parsed = ConvertFrom-EnvLine $_
  if ($parsed) {
    [System.Environment]::SetEnvironmentVariable($parsed.Key, $parsed.Value, 'Process')
  }
}
$revision = (Get-Content $ReleaseFile -Raw)
if (-not $revision -or -not $revision.Trim()) {
  Write-Log 'startup_failed reason=missing_release detail=empty_release_marker'
  [Console]::Error.WriteLine('memory MCP release marker is empty')
  exit 78
}
$revision = $revision.Trim()
[System.Environment]::SetEnvironmentVariable('MEMORY_BUILD_REVISION', $revision, 'Process')

$myPid = $PID
$record = Join-Path $RegistryDir "$myPid.pid"
Set-Content -Path $record -Value $myPid
icacls $record /inheritance:r /grant:r "$($env:USERNAME):F" | Out-Null
if ($LASTEXITCODE -ne 0) {
  Write-Log "acl_grant_failed target=$record exit=$LASTEXITCODE"
}

$parentPid = 'unknown'
try {
  $parentPid = (Get-CimInstance Win32_Process -Filter "ProcessId=$myPid").ParentProcessId
} catch {}
Write-Log "started pid=$myPid parent=$parentPid revision=$revision"

& $ServerExe @ServerArgs
$status = $LASTEXITCODE
Remove-Item -Force $record -ErrorAction SilentlyContinue

if ($status -eq 0) {
  Write-Log "stopped_cleanly pid=$myPid reason=stdio_peer_closed"
} else {
  Write-Log "stopped_with_error pid=$myPid exit=$status"
}
exit $status
