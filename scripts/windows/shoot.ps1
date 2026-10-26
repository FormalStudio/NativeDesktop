# Captures one app window in the logged-in desktop session (scripts/windows/vm shoot). The ndshell
# agent launches the app, since a process started from ssh has no desktop, and captures the window
# as DWM composes it. Same environment as `nd docs capture`: dark, a fresh data dir, automation on.
param(
    [Parameter(Mandatory)][string]$Exe,
    [Parameter(Mandatory)][string]$Out,
    [string[]]$Env = @(),
    [string[]]$Click = @(),
    [double]$Settle = 3,
    [string]$Size = '',
    [string]$Ndshell = 'C:\nd\shots\ndshell.exe'
)
$ErrorActionPreference = 'Stop'
$agent = Join-Path $env:LOCALAPPDATA 'nd\agent\ndshell.exe'
# Windows PowerShell 5.1 drops the double quotes inside an argument to a native exe.
function Agent($request) {
    $reply = & $agent agent call ((ConvertTo-Json $request -Compress -Depth 5) -replace '"', '\"')
    if ($LASTEXITCODE -ne 0) { throw "agent $($request.cmd): $reply" }
    $reply | ConvertFrom-Json
}

$alive = $false
if (Test-Path $agent) { & $agent agent ping 2>&1 | Out-Null; $alive = $LASTEXITCODE -eq 0 }
if (-not $alive) {
    & $Ndshell agent install 2>&1 | ForEach-Object { "$_" }
    Start-Sleep 3
    Agent @{ cmd = 'ping' } | Out-Null
}

$run = Join-Path (Split-Path $Out) ('run-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force "$run\data" | Out-Null
Set-Content -Path "$run\theme.toml" -Value 'appearance = "dark"' -Encoding ascii
$pipe = 'nd-shots-' + (Split-Path $run -Leaf)
$vars = @{
    NATIVE_AUTOMATION    = '1'
    ND_AUTOMATION_SOCKET = "\\.\pipe\$pipe"
    ND_APP_DATA_DIR      = "$run\data"
    ND_THEME_TOML        = "$run\theme.toml"
    ND_THEME_JSON        = ''
}
foreach ($pair in $Env) { $k, $v = $pair -split '=', 2; $vars[$k] = $v }

$launched = Agent @{ cmd = 'launch'; exe = $Exe; env = $vars; cwd = (Split-Path $Exe) }
$appPid = $launched.pid
try {
    $info = Agent @{ cmd = 'window'; pid = $appPid; timeout_ms = 60000 }
    $hwnd = $info.hwnd
    if ($Size) {
        $w, $h = $Size -split 'x'
        $info = Agent @{ cmd = 'set_frame'; hwnd = $hwnd; rect = @($info.frame[0], $info.frame[1], [int]$w, [int]$h) }
    }

    if ($Click.Count -gt 0) {
        # nd automation: JSON-RPC 2.0 frames behind a little-endian u32 length.
        $client = New-Object System.IO.Pipes.NamedPipeClientStream('.', $pipe, [System.IO.Pipes.PipeDirection]::InOut)
        $client.Connect(30000)
        $script:id = 0
        function Rpc($method, $params) {
            $script:id++
            $body = [Text.Encoding]::UTF8.GetBytes((ConvertTo-Json @{ jsonrpc = '2.0'; id = $script:id; method = $method; params = $params } -Compress))
            $client.Write([BitConverter]::GetBytes([uint32]$body.Length), 0, 4)
            $client.Write($body, 0, $body.Length)
            $client.Flush()
            $head = New-Object byte[] 4
            $read = 0
            while ($read -lt 4) { $read += $client.Read($head, $read, 4 - $read) }
            $len = [BitConverter]::ToUInt32($head, 0)
            $buf = New-Object byte[] $len
            $read = 0
            while ($read -lt $len) { $read += $client.Read($buf, $read, $len - $read) }
            [Text.Encoding]::UTF8.GetString($buf) | ConvertFrom-Json
        }
        function Find($node, $testId) {
            if ($node.testId -eq $testId -and $node.visible -and $node.bounds) { return $node }
            foreach ($child in $node.children) { $hit = Find $child $testId; if ($hit) { return $hit } }
        }
        foreach ($testId in $Click) {
            # A pointer click at the node's middle, as nd_test's mouse().click does: rows answer a
            # pointer but not the semantic click. The window shows before the app has loaded its
            # rows, so the node is polled for.
            $deadline = (Get-Date).AddSeconds(30)
            do {
                $reply = Rpc 'getTree' @{}
                if ($reply.error) { throw "getTree: $($reply.error.message)" }
                $node = $null
                foreach ($window in $reply.result.windows) { if (-not $node) { $node = Find $window.root $testId } }
                if (-not $node) { Start-Sleep -Milliseconds 500 }
            } while (-not $node -and (Get-Date) -lt $deadline)
            if (-not $node) { throw "click $testId`: no visible node with that test id" }
            $x = $node.bounds.x + $node.bounds.width / 2
            $y = $node.bounds.y + $node.bounds.height / 2
            foreach ($phase in 'down', 'up') {
                $reply = Rpc 'pointer' @{ phase = $phase; x = $x; y = $y; clickCount = 1 }
                if ($reply.error) { throw "click $testId`: $($reply.error.message)" }
            }
            Start-Sleep -Milliseconds 1500
        }
        $client.Dispose()
    }

    Start-Sleep -Milliseconds ([int]($Settle * 1000))
    # An inactive window draws its title bar and backdrop differently. A click on an empty stretch
    # of caption (WM_NCHITTEST answers HTCAPTION, 2) activates it without pressing a control.
    $info = Agent @{ cmd = 'window_info'; hwnd = $hwnd }
    if (-not $info.foreground) {
        $x, $y, $w = $info.frame[0], $info.frame[1], $info.frame[2]
        :find foreach ($dy in 12, 20, 6, 28) {
            foreach ($step in 0..7) {
                $offset = [math]::Floor(($step + 1) / 2) * [math]::Floor($w / 16) * $(if ($step % 2 -eq 0) { 1 } else { -1 })
                $at = @(($x + [math]::Floor($w / 2) + $offset), ($y + $dy))
                if ((Agent @{ cmd = 'hit_test'; hwnd = $hwnd; at = $at }).code -eq 2) {
                    Agent @{ cmd = 'click'; at = $at } | Out-Null
                    break find
                }
            }
        }
        Start-Sleep -Milliseconds 700
    }
    if (-not (Agent @{ cmd = 'window_info'; hwnd = $hwnd }).foreground) { 'the window is not in the foreground' }
    $shot = Agent @{ cmd = 'capture'; hwnd = $hwnd; out = $Out }
    "captured $($shot.size -join 'x')"
} finally {
    Agent @{ cmd = 'kill'; pid = $appPid } | Out-Null
    Remove-Item -Recurse -Force $run -ErrorAction SilentlyContinue
}
