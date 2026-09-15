<#
  Shared KEY=VALUE line parser for the protected memory-platform env file.
  Both scripts/mcp-transport-guard.ps1 and scripts/install-taskscheduler.ps1
  read this same file and must agree on what a value means -- they used to
  have two independent hand-rolled parsers that silently disagreed on
  quoted values containing '#'. Quoted values run to their matching close
  quote (a '#' inside stays literal); unquoted values end at the first '#'
  (a comment). Mirrors src/config.rs's own env-var handling.
#>

function ConvertFrom-EnvLine {
  param([Parameter(Mandatory)][string]$Line)
  $line = $Line.Trim()
  if (-not $line -or $line.StartsWith('#')) { return $null }
  $eq = $line.IndexOf('=')
  if ($eq -lt 1) { return $null }
  $key = $line.Substring(0, $eq).Trim()
  $value = $line.Substring($eq + 1).Trim()
  if ($value.StartsWith('"') -or $value.StartsWith("'")) {
    $quote = $value[0]
    $closeIdx = $value.IndexOf($quote, 1)
    $value = if ($closeIdx -gt 0) { $value.Substring(1, $closeIdx - 1) } else { $value.Trim($quote) }
  } else {
    $value = ($value -split '#', 2)[0].Trim()
  }
  return @{ Key = $key; Value = $value }
}
