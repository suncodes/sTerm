param([string]$Agent)
# 仅当前 sTerm Shell 内的包装；不写入用户 profile 或 CLI 配置。
function Invoke-sTermAgent {
    param([string]$Kind, [object[]]$NativeArguments)
    $requestHeaders = @{ Authorization = "Bearer " + $env:STERM_SHELL_TOKEN }
    $arguments = @($NativeArguments | ForEach-Object { [string]$_ })
    $launch = Invoke-RestMethod -Uri ($env:STERM_SHELL_BRIDGE + "/launch") -Method Post -Headers $requestHeaders -ContentType "application/json; charset=utf-8" -Body (@{agent=$Kind;cwd=$ExecutionContext.SessionState.Path.CurrentLocation.ToString();args=$arguments} | ConvertTo-Json -Compress) -TimeoutSec 45
    if ($launch.passthrough) {
        if ($launch.message) { Write-Host $launch.message -ForegroundColor DarkGray }
        & $launch.program @arguments
        return
    }
    $process = $null
    try {
        $info = New-Object System.Diagnostics.ProcessStartInfo
        $info.FileName = $launch.program
        $info.Arguments = $launch.commandLine
        $info.WorkingDirectory = $launch.cwd
        $info.UseShellExecute = $false
        foreach ($entry in $launch.env) { $info.EnvironmentVariables[$entry[0]] = $entry[1] }
        $process = [System.Diagnostics.Process]::Start($info)
        if (-not $process) { throw "无法启动 Agent" }
        try {
            Invoke-RestMethod -Uri ($env:STERM_SHELL_BRIDGE + "/started") -Method Post -Headers $requestHeaders -ContentType "application/json; charset=utf-8" -Body (@{runtimeId=$launch.runtimeId;pid=$process.Id} | ConvertTo-Json -Compress) -TimeoutSec 8 | Out-Null
        } catch {
            if (-not $process.HasExited) { $process.Kill() }
            throw
        }
        $process.WaitForExit()
        $global:LASTEXITCODE = $process.ExitCode
    } finally {
        if ($process) { $process.Dispose() }
        try {
            Invoke-RestMethod -Uri ($env:STERM_SHELL_BRIDGE + "/finish") -Method Post -Headers $requestHeaders -ContentType "application/json; charset=utf-8" -Body (@{runtimeId=$launch.runtimeId} | ConvertTo-Json -Compress) -TimeoutSec 8 | Out-Null
        } catch { Write-Verbose "sTerm Agent 状态同步已断开" }
    }
}
if ($Agent) {
    Invoke-sTermAgent -Kind $Agent -NativeArguments $args
} else {
    function global:claude { Invoke-sTermAgent -Kind "claude" -NativeArguments $args }
    function global:codex { Invoke-sTermAgent -Kind "codex" -NativeArguments $args }
}
