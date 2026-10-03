#Requires -Version 7.3
# Inspect saved calibration bytes without modifying the file.

param(
    [Parameter(ValueFromRemainingArguments)]
    [AllowEmptyCollection()]
    [AllowEmptyString()]
    [string[]]$RawArgs = @()
)

$ErrorActionPreference = 'Stop'
if (-not (Get-Command Invoke-TraktAction -ErrorAction SilentlyContinue)) {
    . (Join-Path $PSScriptRoot '../tools/bootstrap.ps1')
}
Invoke-TraktCalibration -Arguments $RawArgs