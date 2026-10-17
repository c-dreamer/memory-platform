<#
  Runs llama.cpp's llama-server as the local embedding backend
  (EMBEDDING_MODEL=llama-cpp, decision #1 in docs/WINDOWS_PORT_SYNTHESIS.md).
  Started at logon by the MemoryPlatformLlamaEmbed scheduled task.

  Reads from the protected env file (same file and parser as
  mcp-transport-guard.ps1):
    LLAMA_SERVER_EXE        full path to llama-server.exe
    LLAMA_EMBED_MODEL_PATH  full path to the Qwen3-Embedding-4B GGUF
    LLAMA_CPP_URL           the URL memory-platform calls; host/port are taken
                            from it so the two cannot drift. Loopback only.

  Server flags, from the Qwen3-Embedding llama.cpp instructions:
    --embedding --pooling last   Qwen3-Embedding uses last-token pooling.
    -c/-b/-ub 8192, -np 1        an embedding input must fit one physical
                                 batch; the default 512-token ubatch rejects
                                 the ~4000-char chunks memory-platform sends.
#>
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'lib\env-file.ps1')
$EnvFile = if ($env:MEMORY_ENV_FILE) { $env:MEMORY_ENV_FILE } else { Join-Path $env:USERPROFILE '.config\memory-platform\memory.env' }
if (-not (Test-Path $EnvFile)) { throw "missing_environment: $EnvFile not found" }

$cfg = @{}
foreach ($line in Get-Content $EnvFile) {
  $parsed = ConvertFrom-EnvLine $line
  if ($parsed) { $cfg[$parsed.Key] = $parsed.Value }
}
foreach ($key in 'LLAMA_SERVER_EXE', 'LLAMA_EMBED_MODEL_PATH', 'LLAMA_CPP_URL') {
  if (-not $cfg[$key]) { throw "$key is not set in $EnvFile" }
}
if (-not (Test-Path $cfg.LLAMA_SERVER_EXE)) { throw "llama-server not found: $($cfg.LLAMA_SERVER_EXE)" }
if (-not (Test-Path $cfg.LLAMA_EMBED_MODEL_PATH)) { throw "model not found: $($cfg.LLAMA_EMBED_MODEL_PATH)" }

$uri = [Uri]$cfg.LLAMA_CPP_URL
if ($uri.Host -notin @('127.0.0.1', 'localhost', '::1', '[::1]')) {
  throw "LLAMA_CPP_URL must be loopback; got host '$($uri.Host)'"
}
$bindHost = if ($uri.Host -eq 'localhost') { '127.0.0.1' } else { $uri.Host.Trim('[', ']') }

$StateHome = if ($env:XDG_STATE_HOME) { Join-Path $env:XDG_STATE_HOME 'memory-platform' } else { Join-Path $env:LOCALAPPDATA 'memory-platform' }
New-Item -ItemType Directory -Force -Path $StateHome | Out-Null
$log = Join-Path $StateHome 'MemoryPlatformLlamaEmbed.log'
$errLog = Join-Path $StateHome 'MemoryPlatformLlamaEmbed.error.log'
# Start-Process redirection overwrites, so keep one previous generation.
foreach ($path in $log, $errLog) {
  if (Test-Path $path) { Move-Item -Force $path "$path.1" }
}

$serverArgs = @(
  '-m', "`"$($cfg.LLAMA_EMBED_MODEL_PATH)`"",
  '--embedding', '--pooling', 'last',
  '--host', $bindHost, '--port', $uri.Port,
  '-c', 8192, '-b', 8192, '-ub', 8192, '-np', 1
)
$proc = Start-Process -FilePath $cfg.LLAMA_SERVER_EXE -ArgumentList $serverArgs -NoNewWindow -Wait -PassThru `
  -RedirectStandardOutput $log -RedirectStandardError $errLog
exit $proc.ExitCode
