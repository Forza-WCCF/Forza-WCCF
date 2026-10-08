param(
    [string]$Exe = 'target/x86_64-pc-windows-msvc/release/wccf-installer.exe',
    [string]$Tag = ''
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)
$metadata = & cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cannot read package metadata.' }
$version = ($metadata.packages | Where-Object name -eq 'wccf-installer').version
if (!$version) { throw 'Missing package version.' }
if (!$Tag) { $Tag = "v$version" }
if ($Tag -cne "v$version") { throw "Tag $Tag does not match package version v$version." }
& "$PSScriptRoot/check-licenses.ps1"
$Exe = (Resolve-Path $Exe).Path
$stage = Join-Path 'target' 'release-package'
New-Item $stage -ItemType Directory -Force | Out-Null
New-Item dist -ItemType Directory -Force | Out-Null
Copy-Item $Exe (Join-Path $stage 'wccf-installer.exe') -Force
Copy-Item $Exe 'dist/wccf-installer.exe' -Force
$docs = @('README.md', 'CHANGELOG.md', 'LICENSE', 'THIRD-PARTY-LICENSES.json')
foreach ($file in $docs) {
    Copy-Item $file (Join-Path $stage $file) -Force
    Copy-Item $file (Join-Path 'dist' $file) -Force
}
# Explicit payload: never archive the repository, media, kit, saves or target tree.
$payload = @('wccf-installer.exe') + $docs
$paths = @($payload | ForEach-Object { Join-Path $stage $_ })
$archive = Join-Path 'dist' "wccf-installer-$Tag-windows-x64.zip"
Compress-Archive -Path $paths -DestinationPath $archive -Force
@('dist/wccf-installer.exe', $archive) | ForEach-Object {
    $hash = (Get-FileHash $_ -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $(Split-Path $_ -Leaf)"
} | Set-Content 'dist/SHA256SUMS.txt' -Encoding ASCII
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead((Convert-Path $archive))
try {
    $difference = Compare-Object ($payload | Sort-Object) ($zip.Entries.FullName | Sort-Object)
    if ($difference) { throw 'Unexpected release archive contents.' }
} finally { $zip.Dispose() }
Write-Output "Prepared $archive"
