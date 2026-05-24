# Flash + RTT helper para el ejemplo blink-stm32f769-disco de Rugus.
#
# Uso:
#   .\tools\flash-stm32f769-disco.ps1            # debug profile
#   .\tools\flash-stm32f769-disco.ps1 release    # release profile
#
# Requiere: probe-rs (cargo install probe-rs-tools --locked) y una
# STM32F769I-DISCO conectada por USB ST-LINK on-board.

param(
    [ValidateSet("debug", "release", "release-dev")]
    [string]$Profile = "debug"
)

$ErrorActionPreference = "Stop"

$repoRoot   = Split-Path -Parent $PSScriptRoot
$exampleDir = Join-Path $repoRoot "examples\blink-stm32f769-disco"

if (-not (Test-Path $exampleDir)) {
    throw "No encuentro el ejemplo en $exampleDir"
}

Push-Location $exampleDir
try {
    $profileFlag = if ($Profile -eq "debug") { @() } else { @("--profile=$Profile") }

    Write-Host "==> Building (profile: $Profile)" -ForegroundColor Cyan
    cargo build @profileFlag

    Write-Host "==> Flashing + RTT" -ForegroundColor Cyan
    cargo run @profileFlag
}
finally {
    Pop-Location
}
