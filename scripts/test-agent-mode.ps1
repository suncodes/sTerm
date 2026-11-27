param([string]$ChromePath, [string]$Python="python", [switch]$KeepArtifacts, [switch]$FocusInteractions)
$ErrorActionPreference="Stop"
if($PSVersionTable.PSVersion.Major -lt 7){throw "请使用 PowerShell 7 (pwsh) 运行隔离验证。"}
$workspace=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
if([IO.Path]::GetPathRoot($workspace) -eq "C:\"){throw "独立验证需要非 C 盘工作区。"}
if(-not $ChromePath){
    $candidates=@("$env:LOCALAPPDATA/Google/Chrome/Application/chrome.exe","$env:ProgramFiles/Google/Chrome/Application/chrome.exe")
    $ChromePath=$candidates | Where-Object {Test-Path -LiteralPath $_ -PathType Leaf} | Select-Object -First 1
}
if(-not $ChromePath -or [IO.Path]::GetFileName($ChromePath) -ne "chrome.exe"){throw "需要 Google Chrome；请用 -ChromePath 指定，不会使用 Edge。"}
& $Python -c "import websocket"
if($LASTEXITCODE){throw "需要已有 Python websocket-client 包；脚本不自动安装。"}
$task=Join-Path $workspace (".tmp/agent-mode-"+[Guid]::NewGuid().ToString("N"))
$mock=$null;$dev=$null;$oldTemp=$env:TEMP;$oldTmp=$env:TMP;$passed=$false
function Start-Isolated([string]$Program,[string[]]$Arguments,[hashtable]$Values){
    $info=[Diagnostics.ProcessStartInfo]::new()
    $info.FileName=$Program;$info.WorkingDirectory=$workspace;$info.UseShellExecute=$false;$info.CreateNoWindow=$true
    foreach($argument in $Arguments){$info.ArgumentList.Add($argument)}
    foreach($key in @($info.Environment.Keys)){
        if($key -match "API_KEY|TOKEN|SECRET|PASSWORD|AUTH|PROXY|CODEX|CLAUDE|ANTHROPIC|PI_CODING"){$info.Environment.Remove($key)|Out-Null}
    }
    foreach($entry in $Values.GetEnumerator()){$info.Environment[$entry.Key]=$entry.Value}
    return [Diagnostics.Process]::Start($info)
}
function Drive([hashtable]$Command){
    $raw=& $Python -X utf8 "$PSScriptRoot/agent-mode-drive.py" "$task" ($Command|ConvertTo-Json -Compress)
    if($LASTEXITCODE){throw "隔离驱动失败：$raw"}
    return ($raw|ConvertFrom-Json).result
}
try{
    foreach($name in @("temp","appdata","localappdata","webview","claude","codex","workspace")){New-Item -ItemType Directory -Path "$task/$name" -Force|Out-Null}
    $env:TEMP="$task/temp";$env:TMP="$task/temp"
    $baseEnv=@{TEMP="$task/temp";TMP="$task/temp";NO_PROXY="127.0.0.1,localhost";PYTHONDONTWRITEBYTECODE="1";PYTHONUTF8="1"}
    $mock=Start-Isolated $Python @("-X","utf8","$PSScriptRoot/mock-agent-model.py",$task) $baseEnv
    $deadline=[DateTime]::UtcNow.AddSeconds(10)
    while(-not(Test-Path -LiteralPath "$task/mock-endpoint.json")){
        if($mock.HasExited -or [DateTime]::UtcNow -gt $deadline){throw "本地模拟模型未启动。"}
        Start-Sleep -Milliseconds 100
    }
    $endpoint=(Get-Content -LiteralPath "$task/mock-endpoint.json" -Encoding UTF8 -Raw|ConvertFrom-Json).endpoint
    @{permissions=@{defaultMode="default"};model="haiku";theme="dark";autoUpdates=$false;cacheWarming="off";quietStartup=$true} |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath "$task/claude/settings.json" -Encoding UTF8
    $projects=@{}
    $projects[("$task/workspace".Replace("\","/"))]=@{hasTrustDialogAccepted=$true}
    $projects[("$task/workspace" -replace "/","\")]=@{hasTrustDialogAccepted=$true}
    @{
        hasTrustDialogAccepted=$true;hasCompletedOnboarding=$true
        customApiKeyResponses=@{approved=@("isolated-placeholder");rejected=@()}
        projects=$projects;hasSeenAutoDefaultNudge=$true;hasResetAutoModeOptInForDefaultOffer=$true
        officialMarketplaceAutoInstallAttempted=$true;officialMarketplaceAutoInstalled=$false
        officialMarketplaceAutoInstallRetryCount=100;officialMarketplaceAutoInstallNextRetryTime=4102444800000
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath "$task/claude/.claude.json" -Encoding UTF8
    $config=@"
model_provider = "sterm_probe"
model = "probe"
approval_policy = "on-request"
sandbox_mode = "read-only"
[model_providers.sterm_probe]
name = "sTerm isolated mock"
base_url = "$endpoint/v1"
wire_api = "responses"
requires_openai_auth = false
[analytics]
enabled = false
[features]
send_message_to_user_async = true
[feedback]
enabled = false
[history]
persistence = "none"
[projects."$($task.Replace('\','/'))/workspace"]
trust_level = "trusted"
[tui]
screen_reader_detection_done = true
"@
    Set-Content -LiteralPath "$task/codex/config.toml" -Value $config -Encoding UTF8
    @{
        productName="sTerm Agent Dev";mainBinaryName="sTerm-Agent-Dev";identifier="io.github.suncodes.sterm.agent-dev"
        bundle=@{active=$false}
        app=@{appDirectoriesOverride="$task/appdata";windows=@(@{label="main";title="sTerm Agent Dev";visible=$false;width=1150;height=760;decorations=$false;dragDropEnabled=$false})}
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath "$task/tauri.agent-dev.conf.json" -Encoding UTF8
    Push-Location "$workspace"
    try{
        & npm run tauri -- build --debug --no-bundle --config "$task/tauri.agent-dev.conf.json"
        if($LASTEXITCODE){throw "独立调试版构建失败。"}
    }finally{Pop-Location}
    $values=$baseEnv.Clone()
    $values.STERM_DEV_DATA_DIR="$task/appdata";$values.STERM_DEV_PROBE="1"
    $values.CODEX_HOME="$task/codex";$values.CLAUDE_CONFIG_DIR="$task/claude"
    $values.ANTHROPIC_BASE_URL=$endpoint;$values.ANTHROPIC_API_KEY="sk-ant-sterm-mod-isolated-placeholder"
    $values.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1";$values.DISABLE_TELEMETRY="1";$values.DISABLE_ERROR_REPORTING="1";$values.DISABLE_AUTOUPDATER="1"
    $values.WEBVIEW2_USER_DATA_FOLDER="$task/webview";$values.APPDATA="$task/appdata";$values.LOCALAPPDATA="$task/localappdata"
    $dev=Start-Isolated "$workspace/src-tauri/target/debug/sTerm-Agent-Dev.exe" @() $values
    $deadline=[DateTime]::UtcNow.AddSeconds(25)
    while(-not(Test-Path -LiteralPath "$task/appdata/probe-ready")){
        if($dev.HasExited){throw "隔离应用已退出：$($dev.ExitCode)"}
        if([DateTime]::UtcNow -gt $deadline){throw "隔离应用未报告驱动就绪。"}
        Start-Sleep -Milliseconds 100
    }
    foreach($row in @(@(900,"claude"),@(901,"codex"),@(902,"claude"),@(903,"codex"))){
        Drive @{action="spawn";tab=$row[0];agent=$row[1];cwd="$task/workspace"}|Out-Null
    }
    $browserArgs=@("-X","utf8","$PSScriptRoot/test-agent-browser.py",$task,$ChromePath)
    if($FocusInteractions){$browserArgs+="--focus-interactions"}
    & $Python @browserArgs
    if($LASTEXITCODE){throw "Chrome Agent 集成验证失败。"}
    $passed=$true
}finally{
    if($dev -and -not $dev.HasExited){
        try{Drive @{action="stop"}|Out-Null}catch{Write-Warning $_.Exception.Message}
        if(-not $dev.WaitForExit(10000)){
            $expectedExe=[IO.Path]::GetFullPath("$workspace/src-tauri/target/debug/sTerm-Agent-Dev.exe")
            if($dev.MainModule.FileName -ne $expectedExe){throw "进程路径不匹配，拒绝结束。"}
            $dev.Kill();$dev.WaitForExit()
        }
    }
    if($mock -and -not $mock.HasExited){$mock.Kill();$mock.WaitForExit()}
    $env:TEMP=$oldTemp;$env:TMP=$oldTmp
    if($KeepArtifacts){Write-Output "已按参数保留验证产物：$task"}
    else{
        $expected=[IO.Path]::GetFullPath((Join-Path $workspace ".tmp"))+[IO.Path]::DirectorySeparatorChar
        if(-not $task.StartsWith($expected,[StringComparison]::OrdinalIgnoreCase)){throw "清理目标超出工作区，拒绝删除。"}
        for($attempt=0;$attempt -lt 5;$attempt++){
            try{Remove-Item -LiteralPath "$task" -Recurse -Force -ErrorAction Stop;break}
            catch{if($attempt -eq 4){Write-Warning "临时产物未能清理：$task；$($_.Exception.Message)"};Start-Sleep -Milliseconds 500}
        }
        if(Test-Path -LiteralPath "$task"){throw "临时目录仍存在：$task"}
    }
}
if($passed){Write-Output "PASS 隔离 sTerm + 两个 Claude + 两个 Codex + Google Chrome；临时进程已退出。"}
