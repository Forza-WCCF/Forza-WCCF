$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)
$bundle = Get-Content THIRD-PARTY-LICENSES.json -Raw | ConvertFrom-Json
if ($bundle.target -ne 'x86_64-pc-windows-msvc') { throw 'Wrong license-bundle target.' }
$notices = @{}
foreach ($library in $bundle.third_party_libraries) {
    if (!$library.licenses -or !$library.license) { throw "Missing notices: $($library.package_name)" }
    foreach ($notice in $library.licenses) {
        if ([string]::IsNullOrWhiteSpace($notice.text) -or $notice.text -eq 'NOT FOUND') {
            throw "Missing license text: $($library.package_name)"
        }
    }
    $notices["$($library.package_name) v$($library.package_version)"] = $true
}
$tree = & cargo tree --locked --target x86_64-pc-windows-msvc -e normal,build --prefix none --format '{p}'
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve Windows dependency tree.' }
foreach ($line in $tree) {
    if ($line -match '^(\S+) v(\S+)') {
        $name = "$($Matches[1]) v$($Matches[2])"
        if ($Matches[1] -ne 'wccf-installer' -and !$notices.ContainsKey($name)) {
            throw "Refresh and review THIRD-PARTY-LICENSES.json: missing $name"
        }
    }
}
Write-Output 'Windows dependency license notices are complete.'
