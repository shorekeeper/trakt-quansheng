#Requires -Version 7.3
# One reserved firmware identity for the selected CI gate.

param(
    [string[]]$Only = @(),
    [string[]]$Skip = @(),
    [string]$Assign = '',
    [switch]$IgnoreIncrement,
    [ValidateSet('full', 'radio', 'compact', 'standalone', 'debug', 'full-fam', 'all')][string]$Variant = 'all'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'tools/bootstrap.ps1')
try {
    $result = Invoke-TraktCi -Only $Only -Skip $Skip -Assign $Assign `
        -IgnoreIncrement:$IgnoreIncrement -Variant $Variant
    Write-TraktResultOutput -Result $result
    exit $result.ExitCode
} catch {
    Write-Host $_.Exception.Message -ForegroundColor Red
    exit 1
}