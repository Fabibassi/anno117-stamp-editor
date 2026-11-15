# Build a release EXE without embedding local home/project paths (panic locations).
$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..")
$crate = Join-Path $repo "stamp_editor"
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }

$env:RUSTFLAGS = "--remap-path-prefix ${crate}=. --remap-path-prefix ${repo}=. --remap-path-prefix ${cargoHome}=.cargo"
Write-Host "RUSTFLAGS=$($env:RUSTFLAGS)"

Push-Location $crate
try {
    cargo build --release
} finally {
    Pop-Location
}

$exe = Join-Path $crate "target\release\stamp_editor.exe"
$bytes = [System.IO.File]::ReadAllBytes($exe)
$text = [System.Text.Encoding]::ASCII.GetString($bytes)
$user = $env:USERNAME
if ($text.Contains($user) -or $text.Contains("Users\$user")) {
    Write-Warning "EXE still contains username '$user' — check remapping."
} else {
    Write-Host "OK: no username strings in EXE"
}

$ver = (Select-String -Path (Join-Path $crate "Cargo.toml") -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$out = Join-Path $repo "dist\stamp_editor-$ver-windows-x64"
Remove-Item -Recurse -Force $out, "$out.zip" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $out | Out-Null
Copy-Item $exe $out
Copy-Item (Join-Path $repo "README.md"), (Join-Path $repo "LICENSE"), (Join-Path $repo "CHANGELOG.md") $out
Compress-Archive -Path "$out\*" -DestinationPath "$out.zip" -Force
Get-FileHash "$out.zip" -Algorithm SHA256 | Format-List
Get-Item "$out.zip" | Select-Object FullName, Length
