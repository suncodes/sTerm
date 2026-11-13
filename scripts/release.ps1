<#
.SYNOPSIS
同步版本、验证、提交、打标签并推送 GitHub Release 所需的 Git 引用。

.DESCRIPTION
默认要求工作区干净，只提交本次版本同步产生的元数据。
如确实要把当前工作区改动一起作为发布内容，必须显式指定 -IncludeWorkingTree。
推送 v<version> 标签后，GitHub Actions 会构建并上传发布产物。

.EXAMPLE
npm run release -- 0.1.4

.EXAMPLE
npm run release -- 0.1.4 -Approve

.EXAMPLE
npm run release -- 0.1.4 -IncludeWorkingTree -Approve
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateNotNullOrEmpty()]
    [string]$Version,

    [switch]$IncludeWorkingTree,

    [switch]$Approve
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Program,

        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "命令执行失败（退出码 $LASTEXITCODE）：$Program $($Arguments -join ' ')"
    }
}

function Get-CheckedOutput {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Program,

        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    $output = @(& $Program @Arguments)
    if ($LASTEXITCODE -ne 0) {
        throw "命令执行失败（退出码 $LASTEXITCODE）：$Program $($Arguments -join ' ')"
    }
    return ($output -join [Environment]::NewLine).Trim()
}

function Get-WorkingTreeChanges {
    $changes = @(& git status --porcelain=v1 --untracked-files=all)
    if ($LASTEXITCODE -ne 0) {
        throw "无法读取 Git 工作区状态。"
    }
    return @($changes | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
}

function Test-TagMissing {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Tag
    )

    & git show-ref --verify --quiet ("refs/tags/" + $Tag)
    if ($LASTEXITCODE -eq 0) {
        throw "标签 $Tag 已存在，发布已取消。"
    }
    if ($LASTEXITCODE -ne 1) {
        throw "无法检查标签 $Tag 是否存在。"
    }
}

function Confirm-Release {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Tag
    )

    if ($Approve) {
        return
    }

    $answer = Read-Host "将创建并推送 $Tag。输入 $Tag 确认"
    if ($answer -cne $Tag) {
        throw "未确认发布，已取消。"
    }
}

function Assert-RemoteMainCanPublish {
    Invoke-Checked "git" @("fetch", "--tags", "origin", "main")

    $localHead = Get-CheckedOutput "git" @("rev-parse", "HEAD")
    $remoteHead = Get-CheckedOutput "git" @("rev-parse", "origin/main")
    $mergeBase = Get-CheckedOutput "git" @("merge-base", "HEAD", "origin/main")

    if ($mergeBase -ne $remoteHead -and $mergeBase -ne $localHead) {
        throw "本地 main 与 origin/main 已分叉。请先人工处理冲突后再发布。"
    }
    if ($mergeBase -eq $localHead -and $localHead -ne $remoteHead) {
        throw "本地 main 落后于 origin/main。请先同步远端提交后再发布。"
    }
    if ($mergeBase -eq $remoteHead -and $localHead -ne $remoteHead) {
        Write-Warning "本地 main 含有尚未推送的提交；它们将与本次发布一起推送。"
    }
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$previousLocation = Get-Location
Push-Location -LiteralPath $projectRoot

try {
    foreach ($command in @("git", "node", "npm", "cargo")) {
        Get-Command $command -ErrorAction Stop | Out-Null
    }

    $repositoryRoot = Get-CheckedOutput "git" @("rev-parse", "--show-toplevel")
    $expectedRoot = [System.IO.Path]::GetFullPath($projectRoot)
    $actualRoot = [System.IO.Path]::GetFullPath($repositoryRoot)
    if (-not $actualRoot.Equals($expectedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "发布脚本必须从 sTerm 仓库根目录运行。"
    }

    $branch = Get-CheckedOutput "git" @("branch", "--show-current")
    if ($branch -ne "main") {
        throw "当前分支为 $branch；仅允许从 main 发布。"
    }

    $origin = Get-CheckedOutput "git" @("remote", "get-url", "origin")
    if ([string]::IsNullOrWhiteSpace($origin)) {
        throw "未配置 origin 远程仓库。"
    }

    # PowerShell 会把单元素输出解包为字符串；强制保留数组，确保后续 .Count 可靠。
    $existingChanges = @(Get-WorkingTreeChanges)
    $changeSummary = $existingChanges -join [Environment]::NewLine
    if ($existingChanges.Count -gt 0 -and -not $IncludeWorkingTree) {
        throw "工作区存在未提交改动。请先提交，或使用 -IncludeWorkingTree 显式将其纳入本次发布：$([Environment]::NewLine)$changeSummary"
    }
    if ($existingChanges.Count -gt 0) {
        Write-Warning "当前工作区改动将全部进入本次发布提交：$([Environment]::NewLine)$changeSummary"
    }

    $inputVersion = $Version.Trim()
    $normalizedInputVersion = if ($inputVersion.StartsWith("v", [System.StringComparison]::Ordinal)) {
        $inputVersion.Substring(1)
    } else {
        $inputVersion
    }
    $releaseTag = "v" + $normalizedInputVersion

    Assert-RemoteMainCanPublish
    Test-TagMissing $releaseTag

    Write-Host "即将发布 $releaseTag 到 $origin。"
    if ($IncludeWorkingTree) {
        Write-Host "模式：包含当前工作区的全部改动。"
    } else {
        Write-Host "模式：仅提交本次版本同步产生的元数据。"
    }
    Confirm-Release $releaseTag

    Invoke-Checked "npm" @("run", "version:set", "--", $normalizedInputVersion)
    $releaseVersion = Get-CheckedOutput "node" @("-p", "require('./package.json').version")
    $releaseTag = "v" + $releaseVersion
    if ($releaseVersion -ne $normalizedInputVersion) {
        throw "版本同步结果与请求不一致：请求 $normalizedInputVersion，得到 $releaseVersion。"
    }

    Invoke-Checked "npm" @("run", "version:check")
    Invoke-Checked "npm" @("test")
    Invoke-Checked "npm" @("run", "build")
    Invoke-Checked "cargo" @("test", "--manifest-path", "src-tauri/Cargo.toml")
    Invoke-Checked "git" @("diff", "--check")

    if ($IncludeWorkingTree) {
        Invoke-Checked "git" @("add", "--all")
    } else {
        Invoke-Checked "git" @(
            "add",
            "--",
            "package.json",
            "package-lock.json",
            "src-tauri/Cargo.toml",
            "src-tauri/Cargo.lock",
            "src-tauri/tauri.conf.json"
        )
    }

    Invoke-Checked "git" @("diff", "--cached", "--check")
    & git diff --cached --quiet
    if ($LASTEXITCODE -eq 0) {
        throw "没有可提交的发布改动。请使用一个尚未发布的新版本号。"
    }
    if ($LASTEXITCODE -ne 1) {
        throw "无法检查暂存区改动。"
    }

    Invoke-Checked "git" @("commit", "-m", ("chore: release " + $releaseTag))
    Invoke-Checked "git" @("tag", "-a", $releaseTag, "-m", $releaseTag)
    Invoke-Checked "git" @(
        "push",
        "--atomic",
        "origin",
        "refs/heads/main:refs/heads/main",
        ("refs/tags/" + $releaseTag + ":refs/tags/" + $releaseTag)
    )

    Write-Host "已推送 main 和 $releaseTag。GitHub Actions 将自动构建并发布安装包与便携包。"
} finally {
    Pop-Location
    Set-Location -LiteralPath $previousLocation
}
