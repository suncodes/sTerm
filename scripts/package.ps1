param(
    [string]$TargetDir = "src-tauri/target"
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$previousTargetDir = $env:CARGO_TARGET_DIR
Push-Location -LiteralPath $projectRoot
try {
    $buildTargetDir = if ([System.IO.Path]::IsPathRooted($TargetDir)) {
        $TargetDir
    } else {
        Join-Path $projectRoot $TargetDir
    }
    $env:CARGO_TARGET_DIR = [System.IO.Path]::GetFullPath($buildTargetDir)
    & npm run tauri build -- --no-bundle
    if ($LASTEXITCODE -ne 0) { throw "构建失败，未更新便携包" }
    $releaseDir = Join-Path $env:CARGO_TARGET_DIR "release"
    $runtimeDir = Join-Path $projectRoot "src-tauri/resources/conpty"
    Copy-Item -LiteralPath (Join-Path $runtimeDir "conpty.dll") -Destination $releaseDir -Force
    Copy-Item -LiteralPath (Join-Path $runtimeDir "OpenConsole.exe") -Destination $releaseDir -Force
    Copy-Item -LiteralPath (Join-Path $runtimeDir "LICENSE") -Destination (Join-Path $releaseDir "conpty-LICENSE.txt") -Force
    $integrationDir = Join-Path $releaseDir "agent-integration"
    New-Item -ItemType Directory -Path $integrationDir -Force | Out-Null
    $integrationNames = @("install.py", "sterm-events.ts", "README.md")
    foreach ($name in $integrationNames) {
        Copy-Item -LiteralPath (Join-Path $projectRoot "integrations/$name") -Destination $integrationDir -Force
    }
    Copy-Item -LiteralPath (Join-Path $projectRoot "LICENSE") -Destination $releaseDir -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot "NOTICE") -Destination $releaseDir -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot "THIRD_PARTY_NOTICES.md") -Destination $releaseDir -Force
    $packageItems = @("sTerm.exe", "conpty.dll", "OpenConsole.exe", "conpty-LICENSE.txt", "LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.md", "agent-integration") |
        ForEach-Object { Join-Path $releaseDir $_ }
    $packagePath = Join-Path $releaseDir "sTerm-portable.zip"
    Compress-Archive -LiteralPath $packageItems -DestinationPath $packagePath -Force
    Get-Item -LiteralPath (Join-Path $releaseDir "sTerm.exe"), $packagePath |
        Select-Object FullName, Length, LastWriteTime
} finally {
    $env:CARGO_TARGET_DIR = $previousTargetDir
    Pop-Location
}
