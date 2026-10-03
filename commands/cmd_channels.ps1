#Requires -Version 7.3
# Export or stage a checked channel bank over UART.

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
Invoke-TraktChannels -Arguments $RawArgs