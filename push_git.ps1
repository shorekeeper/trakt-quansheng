#Requires -Version 7.3
# Verify, confirm staging, commit, and push the configured upstream.

param([string]$Message = '', [switch]$ToolsOnly)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'tools/bootstrap.ps1')

function Invoke-TraktGitRequired {
    param([string[]]$Arguments)
    $result = Invoke-TraktNative -Name git -Arguments (@('-C', $global:TraktRoot) + $Arguments)
    if ($result.ExitCode -ne 0) { throw ($result.Output -join "`n") }
    return $result
}

try {
    $branch = Invoke-TraktGitRequired -Arguments @('symbolic-ref', '--quiet', '--short', 'HEAD')
    if (-not ($branch.Output -join '').Trim()) { throw 'A named branch is required.' }
    $upstream = Invoke-TraktGitRequired -Arguments @(
        'rev-parse', '--abbrev-ref', '--symbolic-full-name', '@{u}'
    )
    Write-Host "Upstream: $($upstream.Output -join '')" -ForegroundColor Cyan

    $status = Invoke-TraktGitRequired -Arguments @('status', '--porcelain')
    if ($status.Output.Count -eq 0) {
        Write-Host 'Working tree is clean; nothing was committed or pushed.'
        exit 0
    }
    foreach ($line in $status.Output) { Write-Host $line }

    $gate = if ($ToolsOnly) { Invoke-TraktCi -Only @('audit', 'tools') } else { Invoke-TraktCi }
    Write-TraktResultOutput -Result $gate
    if ($gate.ExitCode -ne 0) { exit $gate.ExitCode }

    if (-not $Message) { $Message = Read-Host 'Commit message' }
    if ([string]::IsNullOrWhiteSpace($Message)) { throw 'The commit message is empty.' }
    if ((Read-Host 'Stage all changes, commit, and push? Type yes') -cne 'yes') {
        throw 'Push cancelled.'
    }

    $null = Invoke-TraktGitRequired -Arguments @('add', '-A')
    $commit = Invoke-TraktGitRequired -Arguments @('commit', '-m', $Message)
    foreach ($line in $commit.Output) { Write-Host $line }
    $push = Invoke-TraktGitRequired -Arguments @('push')
    foreach ($line in $push.Output) { Write-Host $line }
    exit 0
} catch {
    Write-Host $_.Exception.Message -ForegroundColor Red
    Write-Host 'Any completed local commit remains local if the push failed.' -ForegroundColor Yellow
    exit 1
}