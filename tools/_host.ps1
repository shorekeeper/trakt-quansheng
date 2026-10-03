#Requires -Version 7.3
# Explicit command results, native invocation, and active-source discovery.

function New-TraktResult {
    param(
        [int]$ExitCode = 0,
        [string[]]$Output = @(),
        [string]$Status = '',
        [int]$Passed = 0,
        [int]$Failed = 0,
        [int]$Skipped = 0
    )
    if (-not $Status) { $Status = if ($ExitCode -eq 0) { 'PASS' } else { 'FAIL' } }
    return [pscustomobject]@{
        ExitCode = $ExitCode; Status = $Status; Output = [string[]]$Output
        Passed = $Passed; Failed = $Failed; Skipped = $Skipped
    }
}

function Invoke-TraktNative {
    param([string]$Name, [string[]]$Arguments = @())

    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($null -eq $command) {
        return [pscustomobject]@{ ExitCode = 1; Output = @("Executable not found: $Name") }
    }
    $outputLines = @(& $command.Source @Arguments 2>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
    return [pscustomobject]@{ ExitCode = [int]$code; Output = [string[]]$outputLines }
}

function Get-TraktSourceFiles {
    $files = [System.Collections.Generic.List[object]]::new()
    foreach ($name in @(
        'Cargo.toml', 'Cargo.lock', 'version.json', '.gitignore',
        'shell.ps1', 'ci.ps1', 'push_git.ps1'
    )) {
        $path = Join-Path $global:TraktRoot $name
        if (Test-Path -LiteralPath $path -PathType Leaf) { $files.Add((Get-Item -LiteralPath $path)) }
    }
    foreach ($directory in @('.cargo', 'src', 'link', 'tools', 'commands', 'tests', 'docs')) {
        $path = Join-Path $global:TraktRoot $directory
        if (Test-Path -LiteralPath $path -PathType Container) {
            foreach ($file in Get-ChildItem -LiteralPath $path -Recurse -File) { $files.Add($file) }
        }
    }
    return @($files | Sort-Object FullName -Unique)
}

function Initialize-TraktMuxModules {
    if (-not $IsWindows) { throw 'The multiplexer requires Windows.' }
    if (-not $global:TraktMuxLoaded) {
        throw 'Multiplexer declarations were not loaded by bootstrap.'
    }
    foreach ($name in @('TraktTheme', 'TraktMuxEngine', 'TraktMenuPane')) {
        if ($null -eq ([System.Management.Automation.PSTypeName]$name).Type) {
            throw "Multiplexer type is not visible: $name"
        }
    }
}

function Save-TraktTrace {
    param([string]$Command, [string[]]$Arguments, [object]$Result, [double]$DurationMs)

    $entry = [pscustomobject]@{
        Index = $global:CommandCount
        Timestamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss.fff'
        Command = $Command
        Args = $Arguments -join ' '
        ExitCode = $Result.ExitCode
        Status = $Result.Status
        DurationMs = [math]::Round($DurationMs, 3)
        ErrorCount = if ($Result.ExitCode -eq 0) { 0 } else { 1 }
        WarnCount = 0
        Errors = if ($Result.ExitCode -eq 0) { @() } else { @($Result.Output) }
        Warnings = @()
        OutputTail = @($Result.Output | Select-Object -Last 50)
        BuildStages = if ($null -ne $Result.PSObject.Properties['BuildStages']) {
            @($Result.BuildStages)
        } else {
            @()
        }
    }
    $path = Join-Path $global:TraceDir "$($global:SessionId).json"
    $entries = @()
    if (Test-Path -LiteralPath $path) {
        $entries = @([System.IO.File]::ReadAllText($path) | ConvertFrom-Json)
    }
    $entries += $entry
    $json = ConvertTo-Json -InputObject @($entries) -Depth 8
    $json = [regex]::Replace($json, '[^\x00-\x7F]', {
        param($match)
        '\u{0:x4}' -f [int][char]$match.Value
    })
    [System.IO.File]::WriteAllText($path, $json, [System.Text.Encoding]::ASCII)
}