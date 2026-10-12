param([string]$Exe = 'target/x86_64-pc-windows-msvc/release/wccf-installer.exe')
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)
$Exe = (Resolve-Path $Exe).Path
$root = Join-Path ([IO.Path]::GetTempPath()) ('wccf-installer-smoke-' + [Guid]::NewGuid())
New-Item $root -ItemType Directory | Out-Null
function Invoke-Installer([string[]]$Arguments, [int]$Expected) {
    $quoted = ($Arguments | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $process = Start-Process $Exe -ArgumentList $quoted -PassThru -Wait -NoNewWindow -RedirectStandardOutput (Join-Path $root 'stdout.txt') -RedirectStandardError (Join-Path $root 'stderr.txt')
    if ($process.ExitCode -ne $Expected) {
        throw "Installer returned $($process.ExitCode), expected ${Expected}: $quoted"
    }
}
try {
    Invoke-Installer @('--help') 0
    $help = Get-Content (Join-Path $root 'stdout.txt') -Raw
    if ($help -notmatch 'Forza-WCCF Installer' -or $help -notmatch '--game' -or $help -match 'Revival') { throw 'Wrong branding or edition help.' }
    Invoke-Installer @('--version') 0
    $manifest = Get-Content Cargo.toml -Raw
    if ($manifest -notmatch '(?m)^version = "([^"]+)"') { throw 'Missing package version.' }
    $version = $Matches[1]
    if ((Get-Content (Join-Path $root 'stdout.txt') -Raw).Trim() -ne "Forza-WCCF Installer $version") { throw 'Wrong release version/branding.' }
    Invoke-Installer @('--dest') 1
    Invoke-Installer @('--unknown') 1
    $missing = Join-Path $root 'absent'
    Invoke-Installer @('--dest', $missing) 1
    if (Test-Path $missing) { throw 'Invalid input wrote a destination.' }

    # Synthetic inert files only; no media, keys, kit scripts or game are executed.
    $edition = Join-Path $root '2017-2018'
    $game = Join-Path $edition 'game'
    New-Item (Join-Path $game 'local') -ItemType Directory -Force | Out-Null
    Set-Content (Join-Path $game 'client_Release.exe') 'inert test placeholder' -Encoding ASCII
    Set-Content (Join-Path $game 'local/SDDY_Table.dat') 'SDDY' -Encoding ASCII
    Set-Content (Join-Path $edition '.probe-a') 'untouched' -Encoding ASCII
    Invoke-Installer @('--dest', $root) 0
    $oldEdition = Join-Path $root '2010-2011'
    $oldGame = Join-Path $oldEdition 'game'
    New-Item (Join-Path $oldGame 'local') -ItemType Directory -Force | Out-Null
    Set-Content (Join-Path $oldGame 'client_Release.exe') 'old edition unchanged' -Encoding ASCII
    Set-Content (Join-Path $oldGame 'local/SBWG_Table.dat') 'SBWG' -Encoding ASCII
    foreach ($folder in @($oldEdition, $edition)) {
        New-Item (Join-Path $folder 'data/save') -ItemType Directory -Force | Out-Null
        Set-Content (Join-Path $folder 'data/save/club.bin') $folder -Encoding ASCII
    }
    Invoke-Installer @('--dest', $root) 1 # both present: require an explicit edition
    Invoke-Installer @('--dest', $root, '--game', 'SBWG') 0
    Invoke-Installer @('--dest', $root, '--game', 'SDDY') 0
    Invoke-Installer @('--dest', $root, '--game', 'SDDY', '--english') 1
    Invoke-Installer @('--dest', $root, '--game', 'SDDY', '--iso', 'missing.iso', '--key', 'missing.key') 1
    if ((Get-Content (Join-Path $oldGame 'client_Release.exe') -Raw).Trim() -ne 'old edition unchanged') { throw 'Other edition changed.' }
    foreach ($folder in @($oldEdition, $edition)) {
        if ((Get-Content (Join-Path $folder 'data/save/club.bin') -Raw).Trim() -ne $folder) { throw 'Edition save changed.' }
    }
    if ((Get-Content (Join-Path $game 'client_Release.exe') -Raw).Trim() -ne 'inert test placeholder') {
        throw 'Existing game files changed.'
    }
    if ((Get-Content (Join-Path $edition '.probe-a') -Raw).Trim() -ne 'untouched') {
        throw 'Extraction-only mode ran the hard-link probe.'
    }
    Write-Output 'Headless release smoke check passed; no game was launched.'
} finally {
    Remove-Item $root -Recurse -Force
}
