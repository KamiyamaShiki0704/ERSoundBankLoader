[CmdletBinding()]
param(
    [string]$DllPath,
    [string]$Name = 'ERSoundBankLoader-v0.4.2-shared-bank-fix'
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../../../..')).Path
if ($Name -notmatch '^[A-Za-z0-9][A-Za-z0-9.-]+$') {
    throw 'Name must be a simple delivery name, without directory components.'
}
if (-not $DllPath) {
    $DllPath = Join-Path $projectRoot 'target/shared-boss-bank-fix/x86_64-pc-windows-msvc/release/er_sound_bank_loader.dll'
}
$dll = (Resolve-Path -LiteralPath $DllPath).Path
$stage = Join-Path $projectRoot "dist/$Name"
$zip = "$stage.zip"
if ((Test-Path -LiteralPath $stage) -or (Test-Path -LiteralPath $zip)) {
    throw 'Delivery already exists; inspect it before selecting another name.'
}
$ini = Join-Path $projectRoot 'ersoundbank_loader.ini'
if (Select-String -LiteralPath $ini -Pattern '^\s*(sound_bank_ids?|condition_bank_mappings?|sp_effect_bank_mappings?|sfx_condition_bank_mappings?|sfx_sp_effect_bank_mappings?)\s*=') {
    throw 'Public INI contains active mappings; do not package private configuration.'
}
$relativeFiles = @(
    'README.md', 'LICENSE', 'ersoundbank_loader.ini',
    'Scripts/delivery/sfx-bank-recon/compatibility-1.17.1.md',
    'Scripts/delivery/shared-sound-bank/spec.md',
    'Scripts/delivery/shared-sound-bank/design.md',
    'Scripts/delivery/shared-sound-bank/build.md'
)
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item -LiteralPath $dll -Destination (Join-Path $stage 'er_sound_bank_loader.dll')
foreach ($relative in $relativeFiles) {
    $destination = Join-Path $stage $relative
    New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot $relative) -Destination $destination
}
$fileHashes = [ordered]@{}
foreach ($relative in @('er_sound_bank_loader.dll') + $relativeFiles) {
    $fileHashes[$relative] = (Get-FileHash -LiteralPath (Join-Path $stage $relative) -Algorithm SHA256).Hash
}
$sourceHashes = [ordered]@{}
foreach ($relative in @('Cargo.toml', 'Cargo.lock', '.cargo/config.toml', 'src/lib.rs', 'src/sfx.rs')) {
    $sourceHashes[$relative] = (Get-FileHash -LiteralPath (Join-Path $projectRoot $relative) -Algorithm SHA256).Hash
}
$baseRevision = (& git -C $projectRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve source revision.' }
[ordered]@{
    version = '0.4.2'
    base_revision = $baseRevision
    source = 'working tree; exact source hashes below'
    source_sha256 = $sourceHashes
    file_sha256 = $fileHashes
    validation_reference = 'Scripts/delivery/shared-sound-bank/build.md'
} | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $stage 'manifest.json') -Encoding utf8
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
Get-FileHash -LiteralPath $zip -Algorithm SHA256 | Format-List Algorithm, Hash, Path
