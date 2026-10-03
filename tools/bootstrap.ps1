#Requires -Version 7.3
# Load host functions into the entry script's scope.

$traktToolPath = $PSScriptRoot
$traktProjectPath = [System.IO.Path]::GetFullPath((Join-Path $traktToolPath '..'))
if (Get-Command Invoke-TraktAction -ErrorAction SilentlyContinue) {
    if ($global:TraktRoot -ne $traktProjectPath) {
        throw 'Start a fresh PowerShell process before loading another Trakt project.'
    }
    return
}

$global:TraktRoot = $traktProjectPath
$global:TraktToolDir = $traktToolPath
$global:TraktCommandDir = Join-Path $traktProjectPath 'commands'
$global:TraceDir = Join-Path $traktProjectPath '.trakt_trace'
$global:SessionId = (Get-Date -Format 'yyyyMMdd_HHmmss_fff') + "_$PID"
$global:CommandCount = 0
$global:ErrorCount = 0
$global:LastResult = $null
$global:LastLine = ''
$global:TraktExitRequested = $false
$null = New-Item -ItemType Directory -Path $global:TraceDir -Force

# Class declarations remain visible to commands after bootstrap returns.
# These files declare host types and functions; they do not open the console.
foreach ($name in @(
    '_common.ps1', '_config.ps1', '_input.ps1', '_host.ps1',
    '_registry.ps1', '_readline.ps1', '_rust_strip.ps1',
    '_trace_engine.ps1', '_mux_core.ps1', '_mux_native.ps1',
    '_mux_layout.ps1', '_theme.ps1', '_layout.ps1', '_elf.ps1',
    '_variants.ps1', '_build_ui.ps1', '_build.ps1', '_startup_image.ps1', '_calibration.ps1',
    '_version.ps1', '_shot.ps1', '_channels.ps1', '_actions.ps1'
)) {
    . (Join-Path $traktToolPath $name)
}
$global:TraktMuxLoaded = $true
. (Join-Path $traktProjectPath 'tests/elf_cases.ps1')
$config = Import-TraktConfig
$global:UserAliases = $config.Aliases
$global:StartupLine = $config.Startup