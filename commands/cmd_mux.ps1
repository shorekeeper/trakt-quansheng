# Interactive terminal workspace.
#Requires -Version 7.3

param(
    [Parameter(ValueFromRemainingArguments)]
    [AllowEmptyCollection()]
    [AllowEmptyString()]
    [string[]]$RawArgs = @()
)

if (-not (Get-Command Invoke-TraktAction -ErrorAction SilentlyContinue)) {
    . (Join-Path $PSScriptRoot '../tools/bootstrap.ps1')
}
Initialize-TraktMuxModules
if ($RawArgs.Count -gt 1 -or ($RawArgs.Count -eq 1 -and $RawArgs[0] -ne '--reset')) {
    return (New-TraktResult -ExitCode 2 -Output @('Usage: mux [--reset]'))
}

$null = Restore-MuxTheme

$reset = ($RawArgs -contains '--reset')

Write-CmdHeader "mux" "terminal multiplexer"

Initialize-MuxNative

if (-not ([System.Management.Automation.PSTypeName]'TraktConsole').Type) {
    throw 'Windows console interop is unavailable.'
}

$registry = [ordered]@{
    'Clock'  = @{ Make = { [TraktClockPane]::new() };   Type = 'TraktClockPane' }
    'System' = @{ Make = { [TraktSysInfoPane]::new() }; Type = 'TraktSysInfoPane' }
    'Help'   = @{ Make = { [TraktHelpPane]::new() };    Type = 'TraktHelpPane' }
    'Log'    = @{ Make = { [TraktLogPane]::new() };     Type = 'TraktLogPane' }
    'Menu'   = @{ Make = { [TraktMenuPane]::new() };    Type = 'TraktMenuPane' }
}

$layoutPath = Join-Path $PSScriptRoot '..\.trakt_trace\mux_layout.json'

$script:idc = 0
function New-MuxPaneNode {
    param([object]$PaneObject)
    $script:idc++
    return @{ Kind = 'pane'; Id = "p$script:idc"; Pane = $PaneObject }
}
function New-MuxSplitNode {
    param([string]$Dir, [double]$Ratio, [object]$A, [object]$B)
    return @{ Kind = 'split'; Dir = $Dir; Ratio = $Ratio; A = $A; B = $B }
}

$root = $null
if (-not $reset) {
    $root = Import-MuxLayout -Path $layoutPath -Registry $registry
}
if ($null -eq $root) {
    $clock = New-MuxPaneNode ([TraktClockPane]::new())
    $help  = New-MuxPaneNode ([TraktHelpPane]::new())
    $sys   = New-MuxPaneNode ([TraktSysInfoPane]::new())
    $log   = New-MuxPaneNode ([TraktLogPane]::new())
    $rightCol = New-MuxSplitNode 'v' 0.55 $help $sys
    $topRow   = New-MuxSplitNode 'h' 0.42 $clock $rightCol
    $root     = New-MuxSplitNode 'v' 0.72 $topRow $log
}

$engine = [TraktMuxEngine]::new()
$engine.Root = $root
$engine.Registry = $registry
$engine.FocusId = $engine.FirstPaneId($root)
$engine.LogMsg('trakt-mux started')

Write-Host "    entering multiplexer (Ctrl+Q to quit)" -ForegroundColor DarkGray

$prevEnc = [Console]::OutputEncoding
try {
    try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
    $engine.Run()
}
finally {
    try { [Console]::OutputEncoding = $prevEnc } catch { }
    Export-MuxLayout -Root $engine.Root -Path $layoutPath
}

Write-Host "    multiplexer closed (layout saved)" -ForegroundColor Green

New-TraktResult -Output @('Multiplexer closed; layout saved.')
