param(
    [Parameter(Mandatory = $true)]
    [string]$AppName,
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [Parameter(Mandatory = $true)]
    [string]$AppCrate
)

$ErrorActionPreference = "Stop"

function Find-FxcPath {
    $candidates = @(
        $env:GPUI_FXC_PATH,
        "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe",
        "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.7705\x64\fxc.exe"
    )

    return $candidates |
        Where-Object { $_ -and (Test-Path $_) } |
        Select-Object -First 1
}

function Copy-PackageContents {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceDir,
        [Parameter(Mandatory = $true)]
        [string]$DestinationDir
    )

    New-Item -ItemType Directory -Path $DestinationDir -Force | Out-Null
    Get-ChildItem -LiteralPath $SourceDir -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $DestinationDir -Recurse -Force
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$distDir = Join-Path $repoRoot "dist"
$targetDir = Join-Path $repoRoot "target\release"
$packageName = "$AppName-$Version-windows-x86_64"
$packageDir = Join-Path $distDir $packageName
$packageDirFresh = Join-Path $distDir "$packageName.__fresh"
$zipPath = Join-Path $distDir "$packageName.zip"
$stagingRoot = Join-Path $distDir ".staging"
$stagingDir = Join-Path $stagingRoot "$packageName-$([guid]::NewGuid().ToString('N'))"
$iconSource = Join-Path $repoRoot "assets\icons\app.ico"

New-Item -ItemType Directory -Path $distDir -Force | Out-Null
New-Item -ItemType Directory -Path $stagingDir -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $stagingDir "adb\win") -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $stagingDir "icons") -Force | Out-Null

$fxcPath = Find-FxcPath
if ($fxcPath) {
    $env:GPUI_FXC_PATH = $fxcPath
} else {
    $env:CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS = "true"
}

cargo build --release -p $AppCrate
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Copy-Item -LiteralPath (Join-Path $targetDir "$AppCrate.exe") -Destination (Join-Path $stagingDir "$AppName.exe") -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "adb\win\adb.exe") -Destination (Join-Path $stagingDir "adb\win\adb.exe") -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "adb\win\AdbWinApi.dll") -Destination (Join-Path $stagingDir "adb\win\AdbWinApi.dll") -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "adb\win\AdbWinUsbApi.dll") -Destination (Join-Path $stagingDir "adb\win\AdbWinUsbApi.dll") -Force
Copy-Item -LiteralPath $iconSource -Destination (Join-Path $stagingDir "icons\$AppName.ico") -Force

if (Test-Path $zipPath) {
    Remove-Item -LiteralPath $zipPath -Force
}
Compress-Archive -Path (Join-Path $stagingDir "*") -DestinationPath $zipPath -CompressionLevel Optimal

$updatedDistFolder = $false
try {
    if (Test-Path $packageDirFresh) {
        Remove-Item -LiteralPath $packageDirFresh -Recurse -Force
    }
    Copy-PackageContents -SourceDir $stagingDir -DestinationDir $packageDirFresh
    if (Test-Path $packageDir) {
        Remove-Item -LiteralPath $packageDir -Recurse -Force
    }
    Move-Item -LiteralPath $packageDirFresh -Destination $packageDir
    $updatedDistFolder = $true
} catch {
    if (Test-Path $packageDirFresh) {
        Write-Warning "Unable to refresh $packageDir cleanly. A complete unpacked package was left at $packageDirFresh and the zip package was created successfully at $zipPath."
    } else {
        Write-Warning "Unable to refresh $packageDir because files are in use. The zip package was created successfully at $zipPath."
    }
}

Remove-Item -LiteralPath $stagingDir -Recurse -Force

Write-Host "Created zip package: $zipPath"
if ($updatedDistFolder) {
    Write-Host "Refreshed package folder: $packageDir"
}
