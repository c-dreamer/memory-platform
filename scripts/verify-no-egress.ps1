<#
  Proves (or disproves) that this app makes zero non-loopback network
  connections, per docs/WINDOWS_PORT_SYNTHESIS.md's "How the user can prove
  no egress" section. Default mode is a read-only, repeatable socket check --
  safe to run anytime, including under MEMORY_LOCAL_ONLY=0 where Neon/NVIDIA
  traffic is expected and this will correctly report it.

  Firewall-rule installation is a separate, explicit opt-in
  (-InstallFirewallRules): per this user's own standing instruction, firewall
  changes are his to run deliberately, not something a script does by default.

  Usage:
    powershell -File scripts\verify-no-egress.ps1
    powershell -File scripts\verify-no-egress.ps1 -InstallFirewallRules
#>
param(
  [switch]$InstallFirewallRules
)
$ErrorActionPreference = 'Stop'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$ProcessNames = @('memory-platform', 'mcp-server', 'neon-sync', 'memory-dashboard')

if ($InstallFirewallRules) {
  foreach ($name in $ProcessNames) {
    $exe = Join-Path $Root "target\release\$name.exe"
    if (-not (Test-Path $exe)) {
      Write-Host "Skipping $name : $exe not built"
      continue
    }
    $ruleName = "memory-platform-no-egress-$name"
    if (Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue) {
      Write-Host "Already installed: $ruleName"
      continue
    }
    New-NetFirewallRule -DisplayName $ruleName -Direction Outbound `
      -Program $exe -Action Block -RemoteAddress Internet | Out-Null
    Write-Host "Installed: $ruleName (blocks $exe from any Internet address)"
  }
  exit 0
}

Write-Host "Watching for non-loopback connections from: $($ProcessNames -join ', ')"
Write-Host "Exercise the app now (search, store, embed, archive) in another window; Ctrl+C to stop."
Write-Host ""

$seen = New-Object System.Collections.Generic.HashSet[string]
while ($true) {
  $procs = Get-Process -Name $ProcessNames -ErrorAction SilentlyContinue
  if ($procs) {
    $bad = Get-NetTCPConnection -OwningProcess $procs.Id -ErrorAction SilentlyContinue |
      Where-Object { $_.RemoteAddress -notin @('127.0.0.1', '::1', '0.0.0.0') -and $_.State -ne 'Listen' }
    foreach ($conn in $bad) {
      $key = "$($conn.OwningProcess)|$($conn.RemoteAddress)|$($conn.RemotePort)"
      if ($seen.Add($key)) {
        $procName = (Get-Process -Id $conn.OwningProcess -ErrorAction SilentlyContinue).ProcessName
        Write-Warning "Non-loopback connection: $procName (pid $($conn.OwningProcess)) -> $($conn.RemoteAddress):$($conn.RemotePort) [$($conn.State)]"
      }
    }
  }
  Start-Sleep -Seconds 2
}
