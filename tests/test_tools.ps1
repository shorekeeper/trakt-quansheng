#Requires -Version 7.3
# Host grammar and active-script checks; no target execution.

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '../tools/bootstrap.ps1')
$result = Invoke-TraktToolsTest
foreach ($line in $result.Output) { Write-Host $line }
exit $result.ExitCode