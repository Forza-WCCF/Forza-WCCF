param([string]$Exe = 'target/x86_64-pc-windows-msvc/release/wccf-installer.exe')
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)
$Exe = (Resolve-Path $Exe).Path
$root = Join-Path ([IO.Path]::GetTempPath()) ('wccf-installer-smoke-' + [Guid]::NewGuid())
New-Item $root -ItemType Directory | Out-Null
function Invoke-Installer([string[]]$Arguments, [int]$Expected) {
    $quoted = ($Arguments | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $process = Start-Process $Exe -ArgumentList $quoted -PassThru -Wait -NoNewWindow
    if ($process.ExitCode -ne $Expected) {
        throw "Installer returned $($process.ExitCode), expected ${Expected}: $quoted"
    }
}
try {
    Invoke-Installer @('--help') 0
    Invoke-Installer @('--version') 0
    Invoke-Installer @('--dest') 1
    Invoke-Installer @('--unknown') 1
    $missing = Join-Path $root 'absent'
    Invoke-Installer @('--dest', $missing) 1
    if (Test-Path $missing) { throw 'Invalid input wrote a destination.' }

    # Synthetic inert files only; no media, keys, kit scripts or game are executed.
    $game = Join-Path $root 'wccf-revival'
    New-Item (Join-Path $game 'local') -ItemType Directory -Force | Out-Null
    Set-Content (Join-Path $game 'client_Release.exe') 'inert test placeholder' -Encoding ASCII
    Set-Content (Join-Path $game 'local/SDDY_Table.dat') 'SDDY' -Encoding ASCII
    Set-Content (Join-Path $root '.probe-a') 'untouched' -Encoding ASCII
    Invoke-Installer @('--dest', $root) 0
    Invoke-Installer @('--dest', $root, '--english') 1
    Invoke-Installer @('--dest', $root, '--iso', 'missing.iso', '--key', 'missing.key') 1
    if ((Get-Content (Join-Path $game 'client_Release.exe') -Raw).Trim() -ne 'inert test placeholder') {
        throw 'Existing game files changed.'
    }
    if ((Get-Content (Join-Path $root '.probe-a') -Raw).Trim() -ne 'untouched') {
        throw 'Extraction-only mode ran the hard-link probe.'
    }
    Write-Output 'Headless release smoke check passed; no game was launched.'
} finally {
    Remove-Item $root -Recurse -Force
}
