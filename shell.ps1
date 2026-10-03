#Requires -Version 7.3
# Trakt shell. Supports interactive input or one explicitly supplied command line.

param(
    [Parameter(ValueFromRemainingArguments)]
    [string[]]$DirectArgs
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'tools/bootstrap.ps1')

function Write-TraktPrompt {
    Write-Host -NoNewline 'trakt' -ForegroundColor Cyan
    $branch = Invoke-TraktNative -Name git -Arguments @('-C', $global:TraktRoot, 'branch', '--show-current')
    if ($branch.ExitCode -eq 0 -and $branch.Output.Count -gt 0) {
        $name = $branch.Output[0].Trim()
        if ($name) { Write-Host -NoNewline " $name" -ForegroundColor Green }
        $status = Invoke-TraktNative -Name git -Arguments @('-C', $global:TraktRoot, 'status', '--porcelain')
        if ($status.ExitCode -eq 0 -and $status.Output.Count -gt 0) {
            Write-Host -NoNewline " ~$($status.Output.Count)" -ForegroundColor Yellow
        }
    }
    if ($global:ErrorCount -gt 0) {
        Write-Host -NoNewline " $($global:ErrorCount)err" -ForegroundColor Red
    }
    Write-Host -NoNewline '> ' -ForegroundColor DarkGray
}

function Invoke-TraktShellCommand {
    param([string]$Name, [string[]]$Arguments = @())

    $global:CommandCount++
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        if (-not $global:TraktCommands.Contains($Name)) {
            $result = New-TraktResult -ExitCode 2 -Output @("Unknown command: $Name")
        } else {
            $path = Join-Path $global:TraktCommandDir "cmd_$Name.ps1"
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
                throw "Required command adapter is missing: $Name"
            }
            $returned = @(& $path -RawArgs $Arguments)
            if ($returned.Count -ne 1 -or $null -eq $returned[0].PSObject.Properties['ExitCode']) {
                throw "Invalid command result: $Name"
            }
            $result = $returned[0]
        }
    } catch {
        $result = New-TraktResult -ExitCode 1 -Output @($_.Exception.Message)
    }
    $watch.Stop()

    Write-TraktResultOutput -Result $result
    $color = if ($result.Status -eq 'BLOCKED') { 'Yellow' }
             elseif ($result.ExitCode -ne 0) { 'Red' } else { 'Green' }
    Write-Host "  $($result.Status) | $(Format-Duration $watch.Elapsed.TotalMilliseconds)" -ForegroundColor $color
    if ($result.ExitCode -ne 0) { $global:ErrorCount++ }

    $global:LastResult = $result
    try {
        Save-TraktTrace -Command $Name -Arguments $Arguments -Result $result -DurationMs $watch.Elapsed.TotalMilliseconds
    } catch {
        Write-Host "  Trace write failed: $($_.Exception.Message)" -ForegroundColor Yellow
    }
}

function Invoke-TraktLine {
    param([string]$Line, [int]$Depth = 0)

    if ($Depth -gt 8) { throw 'Alias or repeat recursion exceeded the allowed depth.' }
    $segments = @(Split-TraktCommandChain -Line $Line)
    $lastOk = $true

    foreach ($segment in $segments) {
        $execute = switch ($segment.Op) {
            first { $true }
            ';' { $true }
            '&&' { $lastOk }
            '||' { -not $lastOk }
        }
        if (-not $execute) { continue }

        $tokens = @(Split-TraktTokens -Line $segment.Text)
        if ($tokens.Count -eq 0) { continue }
        $name = $tokens[0].ToLowerInvariant()
        [string[]]$arguments = @()
        if ($tokens.Count -gt 1) {
            $arguments = [string[]]$tokens[1..($tokens.Count - 1)]
        }

        if ($global:UserAliases.ContainsKey($name)) {
            $tail = @($arguments | ForEach-Object { ConvertTo-TraktToken $_ }) -join ' '
            $expanded = ($global:UserAliases[$name] + ' ' + $tail).Trim()
            Invoke-TraktLine -Line $expanded -Depth ($Depth + 1)
        } else {
            if ($global:Aliases.ContainsKey($name)) { $name = $global:Aliases[$name] }
            switch ($name) {
                exit {
                    $global:TraktExitRequested = $true
                    $global:LastResult = New-TraktResult
                }
                clear {
                    if (-not [Console]::IsOutputRedirected) { [Console]::Clear() }
                    $global:LastResult = New-TraktResult
                }
                repeat {
                    if ($global:LastLine) {
                        Invoke-TraktLine -Line $global:LastLine -Depth ($Depth + 1)
                    } else {
                        $global:LastResult = New-TraktResult -ExitCode 2 -Output @('Nothing to repeat.')
                        Write-Host 'Nothing to repeat.' -ForegroundColor Yellow
                    }
                }
                default { Invoke-TraktShellCommand -Name $name -Arguments $arguments }
            }
        }
        $lastOk = $null -ne $global:LastResult -and $global:LastResult.ExitCode -eq 0
        if ($global:TraktExitRequested) { break }
    }
}

function Invoke-TraktInputLine {
    param([string]$Line)

    try { Invoke-TraktLine -Line $Line } catch {
        $global:LastResult = New-TraktResult -ExitCode 2 -Output @($_.Exception.Message)
        $global:ErrorCount++
        Write-Host "Input error: $($_.Exception.Message)" -ForegroundColor Red
    }
    if ($Line.Trim() -notmatch '^(repeat|!!)(\s|$)') { $global:LastLine = $Line }
}

Push-Location -LiteralPath $global:TraktRoot
try {
    if ($DirectArgs.Count -gt 0) {
        $line = if ($DirectArgs.Count -eq 1) { $DirectArgs[0] }
                else { @($DirectArgs | ForEach-Object { ConvertTo-TraktToken $_ }) -join ' ' }
        Invoke-TraktInputLine -Line $line
        $code = if ($null -eq $global:LastResult) { 0 } else { $global:LastResult.ExitCode }
    } else {
        if (-not [Console]::IsInputRedirected) {
            Write-Host "`nTRAKT COMMAND SHELL" -ForegroundColor Cyan
            Write-Host 'Ctrl+P palette | Ctrl+R history | help for commands' -ForegroundColor DarkGray
            Write-Host 'UV-K5 V3 / PY32F071 | build, validate, inspect, and channel exchange' -ForegroundColor DarkGray
        }
        if ($global:StartupLine) { Invoke-TraktInputLine -Line $global:StartupLine }

        while (-not $global:TraktExitRequested) {
            if ([Console]::IsInputRedirected) {
                $line = [Console]::In.ReadLine()
                if ($null -eq $line) { break }
            } else {
                Write-TraktPrompt
                $line = Read-TraktLine -RenderPrompt { Write-TraktPrompt }
            }
            if (-not [string]::IsNullOrWhiteSpace($line)) { Invoke-TraktInputLine -Line $line }
        }
        Write-Host "Session: $($global:CommandCount) commands, $($global:ErrorCount) failures." -ForegroundColor DarkGray
        $code = if ($null -eq $global:LastResult) { 0 } else { $global:LastResult.ExitCode }
    }
} finally { Pop-Location }

exit $code