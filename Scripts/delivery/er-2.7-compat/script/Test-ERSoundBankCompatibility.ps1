param(
    [Parameter(Mandatory = $false)]
    [string]$GameExe = "F:\SteamLibrary\steamapps\common\ELDEN RING\Game\eldenring.exe"
)

$ErrorActionPreference = "Stop"
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..\..")).Path
$gamePath = (Resolve-Path -LiteralPath $GameExe).Path
$sourcePath = Join-Path $projectRoot "src\lib.rs"
$source = Get-Content -LiteralPath $sourcePath -Raw

$forbiddenSourcePatterns = @(
    "LOAD_BANK_MEMORY_COPY_FN_RVA",
    "UNLOAD_BANK_FN_RVA",
    "FILECAP_REQUEST_FN_RVA",
    "CSTaskImp::wait_for_instance",
    ".run_recurring("
)

foreach ($pattern in $forbiddenSourcePatterns) {
    if ($source.Contains($pattern)) {
        throw "Compatibility regression: source still contains '$pattern'."
    }
}

$version = (Get-Item -LiteralPath $gamePath).VersionInfo.FileVersion
$hash = (Get-FileHash -LiteralPath $gamePath -Algorithm SHA256).Hash
Write-Host "Checking Elden Ring executable: $gamePath"
Write-Host "File version: $version"
Write-Host "SHA-256: $hash"

$previousGameExe = $env:ER_GAME_EXE
try {
    $env:ER_GAME_EXE = $gamePath
    & cargo test tests::runtime_symbols_resolve_from_configured_game_executable -- --exact --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw "Runtime symbol compatibility test failed with exit code $LASTEXITCODE."
    }
}
finally {
    $env:ER_GAME_EXE = $previousGameExe
}

Write-Host "ERSoundBankLoader static compatibility probe passed."
