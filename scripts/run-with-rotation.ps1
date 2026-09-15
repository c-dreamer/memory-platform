<#
  Runs one Memory Platform bash runner script under Git Bash, appending its
  stdout/stderr to a state-dir log file. Windows Task Scheduler's Exec action
  has no StandardOutPath/StandardErrorPath equivalent (unlike launchd/systemd),
  so every taskscheduler/*.xml.template routes its Action through this script
  instead of calling bash.exe directly (item 9, docs/WINDOWS_PORT_SYNTHESIS.md).

  ponytail: single-generation rotation (current file -> .1, old .1 discarded)
  once a log exceeds -MaxBytes. Not a full logrotate port -- upgrade to
  multi-generation only if a real need to keep more than one rotation shows up.
#>
param(
  [Parameter(Mandatory)][string]$LogName,
  [Parameter(Mandatory)][string]$Bash,
  [Parameter(Mandatory)][string]$Runner,
  [string[]]$RunnerArgs = @(),
  [string]$StateHome = $(Join-Path $env:LOCALAPPDATA 'memory-platform'),
  [long]$MaxBytes = 5MB
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $StateHome | Out-Null

function Rotate-IfLarge([string]$Path) {
  if ((Test-Path $Path) -and (Get-Item $Path).Length -gt $MaxBytes) {
    $old = "$Path.1"
    Remove-Item -Force $old -ErrorAction SilentlyContinue
    Rename-Item -Force $Path $old
  }
}

$log = Join-Path $StateHome "$LogName.log"
$errLog = Join-Path $StateHome "$LogName.error.log"
Rotate-IfLarge $log
Rotate-IfLarge $errLog

& $Bash $Runner @RunnerArgs 1>> $log 2>> $errLog
exit $LASTEXITCODE
