#Requires -Version 5.1
$ErrorActionPreference = "Stop"

$Repo = if ($env:OPENMI_REPO) { $env:OPENMI_REPO } else { "LOWENDLAGANI/Openmi-cli" }
$Version = if ($env:OPENMI_VERSION) { $env:OPENMI_VERSION } else { "latest" }
$Asset = "openmi-x86_64-pc-windows-msvc.zip"

$BaseUrl = if ($Version -eq "latest") {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$Version"
}

$InstallDir = if ($env:OPENMI_INSTALL_DIR) {
    $env:OPENMI_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA "openmi\bin"
}

$WorkDir = Join-Path ([System.IO.Path]::GetTempPath()) ("openmi-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null

try {
    $ArchivePath = Join-Path $WorkDir $Asset
    Write-Host "Downloading $Asset from $BaseUrl..."
    Invoke-WebRequest -Uri "$BaseUrl/$Asset" -OutFile $ArchivePath

    $SumsPath = Join-Path $WorkDir "SHA256SUMS.txt"
    try {
        Invoke-WebRequest -Uri "$BaseUrl/SHA256SUMS.txt" -OutFile $SumsPath
        $Match = Select-String -Path $SumsPath -Pattern " $Asset$" | Select-Object -First 1
        if ($Match) {
            $Expected = ($Match.Line -split "\s+")[0].ToLower()
            $Actual = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToLower()
            if ($Actual -ne $Expected) {
                throw "Checksum mismatch for $Asset (expected $Expected, got $Actual)"
            }
            Write-Host "Verified SHA-256 checksum."
        }
    } catch {
        Write-Warning "Skipping checksum verification: $_"
    }

    Expand-Archive -Path $ArchivePath -DestinationPath $WorkDir -Force
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Path (Join-Path $WorkDir "openmi.exe") -Destination (Join-Path $InstallDir "openmi.exe") -Force

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($UserPath -notlike "*$InstallDir*") {
        $NewPath = if ($UserPath) { "$UserPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        Write-Host "Added $InstallDir to your user PATH (restart your terminal to apply)."
    }

    Write-Host "Installed openmi to $(Join-Path $InstallDir 'openmi.exe')"
    Write-Host "Run 'openmi login' followed by 'openmi' or 'openmi run `"hello`"' to get started."
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $WorkDir
}
