#Requires -Version 7.3
# Capture adapter arguments without executing a command.

param(
    [Parameter(ValueFromRemainingArguments)]
    [AllowEmptyCollection()]
    [AllowEmptyString()]
    [string[]]$RawArgs = @()
)

[pscustomobject]@{
    Count = $RawArgs.Count
    Values = [string[]]$RawArgs
}