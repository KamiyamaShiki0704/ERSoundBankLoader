param(
    [Parameter(Mandatory = $true)]
    [string]$BaseConfig,
    [string]$Name = ("ERSoundBankLoader-sfx-experiment-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
)

$ErrorActionPreference = "Stop"
if ($Name -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]+$') {
    throw "Name must be a plain package name."
}
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "../../../..")).Path
$basePath = (Resolve-Path -LiteralPath $BaseConfig).Path
$originalHash = (Get-FileHash -LiteralPath $basePath -Algorithm SHA256).Hash
$dll = Join-Path $root "target/sfx-experiment/x86_64-pc-windows-msvc/release/er_sound_bank_loader.dll"
$compiledInputs = @("src/lib.rs", "src/sfx.rs", "Cargo.toml", "Cargo.lock", ".cargo/config.toml")
$sourceFiles = $compiledInputs + @(
    "README.md",
    "ersoundbank_loader.ini",
    "Scripts/delivery/sfx-bank-recon/spec.md",
    "Scripts/delivery/sfx-bank-recon/design.md",
    "Scripts/delivery/sfx-bank-recon/EXPERIMENT.md"
)
$dllItem = Get-Item -LiteralPath $dll
foreach ($relative in $compiledInputs) {
    if ((Get-Item -LiteralPath (Join-Path $root $relative)).LastWriteTimeUtc -gt $dllItem.LastWriteTimeUtc) {
        throw "Release DLL is older than $relative; rebuild the isolated experiment first."
    }
}
$folder = Join-Path $root "dist/$Name"
$zip = "$folder.zip"
if ((Test-Path -LiteralPath $folder) -or (Test-Path -LiteralPath $zip)) {
    throw "Package destination already exists; choose another Name."
}
New-Item -ItemType Directory -Path $folder | Out-Null
Copy-Item -LiteralPath $dll -Destination (Join-Path $folder "er_sound_bank_loader.dll")
Copy-Item -LiteralPath (Join-Path $root "LICENSE") -Destination (Join-Path $folder "LICENSE")
Copy-Item -LiteralPath (Join-Path $root "README.md") -Destination (Join-Path $folder "README.md")
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "../EXPERIMENT.md") -Destination (Join-Path $folder "EXPERIMENT.md")

# Keep all existing sound settings. Only replace experimental SFX keys, then enable test logging.
$lines = [IO.File]::ReadAllLines($basePath) | Where-Object {
    $_ -notmatch '^\s*(sfx_enabled|experimental_sfx_enabled|sfx_condition_bank_mapping|sfx_sp_atkcategory_bank_mapping|sfx_sp_effect_bank_mapping|sfx_speffect_bank_mapping|sfx_ready_timeout_ms)\s*='
}
$contents = ($lines -join "`r`n") + @"


# Private, opt-in offline SFX experiment. Not a public default configuration.
[sfx_experiment]
sfx_enabled=true
sfx_ready_timeout_ms=10000
# Repeat these lines to add your own mappings:
# sfx_condition_bank_mapping=<sp_atkcategory>:<SFX-Resource-Bank-ID>
# sfx_sp_effect_bank_mapping=<SpEffect-ID>:<SFX-Resource-Bank-ID>
sfx_condition_bank_mapping=86:2120
log_enabled=true

"@
[IO.File]::WriteAllText((Join-Path $folder "ersoundbank_loader.ini"), $contents, [Text.UTF8Encoding]::new($false))

$sources = [ordered]@{}
foreach ($relative in $sourceFiles) {
    $sources[$relative] = (Get-FileHash -LiteralPath (Join-Path $root $relative) -Algorithm SHA256).Hash
}
$files = [ordered]@{}
foreach ($item in Get-ChildItem -LiteralPath $folder -File) {
    $files[$item.Name] = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash
}
$manifest = [ordered]@{
    package = $Name
    private_test_config = $true
    base_config_sha256 = $originalHash
    executable_sha256 = "D1A84083C6C7C7902162FF098F7D86812839AA6B3575959398857E539C488134"
    spec = "Scripts/delivery/sfx-bank-recon/spec.md@2.0"
    design = "Scripts/delivery/sfx-bank-recon/design.md@2.0"
    native_runtime_acceptance = "NOT RUN; user performs visual and gameplay acceptance"
    source_sha256 = $sources
    files_sha256 = $files
}
[IO.File]::WriteAllText((Join-Path $folder "manifest.json"), ($manifest | ConvertTo-Json -Depth 5), [Text.UTF8Encoding]::new($false))
Compress-Archive -LiteralPath @((Get-ChildItem -LiteralPath $folder -File).FullName) -DestinationPath $zip

$archive = [IO.Compression.ZipFile]::OpenRead($zip)
try {
    foreach ($entry in $archive.Entries) {
        $stream = $entry.Open()
        try { $actual = (Get-FileHash -InputStream $stream -Algorithm SHA256).Hash }
        finally { $stream.Dispose() }
        $expected = (Get-FileHash -LiteralPath (Join-Path $folder $entry.FullName) -Algorithm SHA256).Hash
        if ($actual -ne $expected) { throw "Archive verification failed for $($entry.FullName)." }
    }
    if ($archive.Entries.Count -ne 6) { throw "Unexpected archive file count." }
}
finally { $archive.Dispose() }
if ((Get-FileHash -LiteralPath $basePath -Algorithm SHA256).Hash -ne $originalHash) {
    throw "Original configuration changed while packaging."
}
[pscustomobject]@{
    Folder = $folder
    Zip = $zip
    ZipSHA256 = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
    DllSHA256 = $files["er_sound_bank_loader.dll"]
    DllBytes = $dllItem.Length
    OriginalConfigUnchanged = $true
} | Format-List
