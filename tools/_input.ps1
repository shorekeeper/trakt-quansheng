#Requires -Version 7.3
# Restricted shell grammar. Input is tokenized, not evaluated.

function Split-TraktCommandChain {
    param([string]$Line, [switch]$AllowIncomplete)

    $segments = [System.Collections.Generic.List[object]]::new()
    $buffer = [System.Text.StringBuilder]::new()
    $quote = [char]0
    $operator = 'first'

    for ($i = 0; $i -lt $Line.Length; $i++) {
        $character = $Line[$i]
        if ($character -eq '`' -and $quote -ne "'") {
            [void]$buffer.Append($character)
            if ($i + 1 -lt $Line.Length) {
                $i++
                [void]$buffer.Append($Line[$i])
            } elseif (-not $AllowIncomplete) { throw 'Trailing escape character.' }
            continue
        }
        if ($quote -ne [char]0) {
            [void]$buffer.Append($character)
            if ($character -eq $quote) {
                if ($quote -eq "'" -and $i + 1 -lt $Line.Length -and $Line[$i + 1] -eq "'") {
                    $i++
                    [void]$buffer.Append($Line[$i])
                } else { $quote = [char]0 }
            }
            continue
        }
        if ($character -eq '"' -or $character -eq "'") {
            $quote = $character
            [void]$buffer.Append($character)
            continue
        }

        $nextOperator = ''
        $width = 1
        if ($character -eq ';') { $nextOperator = ';' }
        elseif ($i + 1 -lt $Line.Length) {
            $pair = $Line.Substring($i, 2)
            if ($pair -eq '&&' -or $pair -eq '||') { $nextOperator = $pair; $width = 2 }
        }
        if ($nextOperator) {
            $text = $buffer.ToString().Trim()
            if (-not $text) { throw 'Empty command before a chain operator.' }
            $segments.Add(@{ Op = $operator; Text = $text })
            [void]$buffer.Clear()
            $operator = $nextOperator
            $i += $width - 1
        } else { [void]$buffer.Append($character) }
    }

    if ($quote -ne [char]0 -and -not $AllowIncomplete) { throw 'Unterminated quoted token.' }
    $last = $buffer.ToString().Trim()
    if ($last) { $segments.Add(@{ Op = $operator; Text = $last }) }
    elseif ($operator -ne 'first') {
        if ($AllowIncomplete) { $segments.Add(@{ Op = $operator; Text = '' }) }
        else { throw 'Missing command after a chain operator.' }
    }
    return $segments.ToArray()
}

function Split-TraktTokens {
    param([string]$Line, [switch]$AllowIncomplete)

    $tokens = [System.Collections.Generic.List[string]]::new()
    $buffer = [System.Text.StringBuilder]::new()
    $quote = [char]0
    $active = $false

    for ($i = 0; $i -lt $Line.Length; $i++) {
        $character = $Line[$i]
        if ($character -eq '`' -and $quote -ne "'") {
            $active = $true
            if ($i + 1 -ge $Line.Length) {
                if (-not $AllowIncomplete) { throw 'Trailing escape character.' }
            } else {
                $i++
                [void]$buffer.Append($Line[$i])
            }
            continue
        }
        if ($quote -ne [char]0) {
            if ($character -eq $quote) {
                if ($quote -eq "'" -and $i + 1 -lt $Line.Length -and $Line[$i + 1] -eq "'") {
                    $i++
                    [void]$buffer.Append("'")
                } else { $quote = [char]0 }
            } else { [void]$buffer.Append($character) }
            continue
        }
        if ($character -eq '"' -or $character -eq "'") {
            $quote = $character
            $active = $true
        } elseif ([char]::IsWhiteSpace($character)) {
            if ($active) {
                $tokens.Add($buffer.ToString())
                [void]$buffer.Clear()
                $active = $false
            }
        } else {
            $active = $true
            [void]$buffer.Append($character)
        }
    }

    if ($quote -ne [char]0 -and -not $AllowIncomplete) { throw 'Unterminated quoted token.' }
    if ($active) { $tokens.Add($buffer.ToString()) }
    return $tokens.ToArray()
}

function ConvertTo-TraktToken {
    param([AllowEmptyString()][string]$Text)
    return "'" + $Text.Replace("'", "''") + "'"
}