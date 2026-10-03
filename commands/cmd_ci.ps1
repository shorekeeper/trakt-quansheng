#Requires -Version 7.3
# Dispatch through the existing command registry and result contract.

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
Invoke-TraktAction -Name 'ci' -Arguments $RawArgs