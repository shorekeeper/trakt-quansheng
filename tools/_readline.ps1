# Line editing, history, completion, and command palette.
#Requires -Version 7.3






function Get-TraktCommandNames {
    Get-ChildItem (Join-Path $global:TraktCommandDir 'cmd_*.ps1') -ErrorAction SilentlyContinue |
        ForEach-Object { $_.BaseName.Substring(4) }
}

function Get-TraktHistoryPath {
    Join-Path $PSScriptRoot '..\.trakt_trace\shell_history.txt'
}

function Get-TraktHistory {
    if (-not $global:TraktHistory) {
        $global:TraktHistory = [System.Collections.Generic.List[string]]::new()
        $p = Get-TraktHistoryPath
        if (Test-Path $p) {
            foreach ($l in (Get-Content $p -ErrorAction SilentlyContinue | Select-Object -Last 500)) {
                if ($l) { $global:TraktHistory.Add($l) }
            }
        }
    }
    return ,$global:TraktHistory
}

function Add-TraktHistory {
    param([string]$Line)
    if (-not $Line -or -not $Line.Trim()) { return }
    $null = Get-TraktHistory
    $h = $global:TraktHistory
    if ($h.Count -gt 0 -and $h[$h.Count - 1] -eq $Line) { return }
    $h.Add($Line)
    try {
        $dir = Split-Path (Get-TraktHistoryPath)
        if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
        Add-Content -Path (Get-TraktHistoryPath) -Value $Line -Encoding UTF8
    } catch { }
}


function Get-FuzzyMatch {
    
    param([string]$Pattern, [string]$Text)
    if (-not $Pattern) { return @{ Score = 0; Indices = @() } }
    if (-not $Text) { return $null }
    $p = $Pattern.ToLowerInvariant()
    $t = $Text.ToLowerInvariant()
    $indices = [System.Collections.Generic.List[int]]::new()
    $score = 0
    $ti = 0
    $prev = -2
    for ($pi = 0; $pi -lt $p.Length; $pi++) {
        $found = $t.IndexOf($p[$pi], $ti)
        if ($found -lt 0) { return $null }
        $score += 1
        if ($found -eq ($prev + 1)) { $score += 3 }
        if ($found -eq 0) { $score += 2 }
        elseif ($t[$found - 1] -eq ' ' -or $t[$found - 1] -eq '-' -or $t[$found - 1] -eq '_' -or $t[$found - 1] -eq '.') { $score += 2 }
        $indices.Add($found)
        $prev = $found
        $ti = $found + 1
    }
    $score -= [int]($t.Length / 10)
    return @{ Score = $score; Indices = $indices.ToArray() }
}


function Get-PrevWordStart {
    
    param([string]$Buf, [int]$Cur)
    $i = $Cur
    while ($i -gt 0 -and $Buf[$i - 1] -eq ' ') { $i-- }
    while ($i -gt 0 -and $Buf[$i - 1] -ne ' ') { $i-- }
    return $i
}

function Get-NextWordEnd {
    
    param([string]$Buf, [int]$Cur)
    $n = $Buf.Length
    $i = $Cur
    while ($i -lt $n -and $Buf[$i] -eq ' ') { $i++ }
    while ($i -lt $n -and $Buf[$i] -ne ' ') { $i++ }
    return $i
}

function Get-CommonPrefix {
    
    param([string[]]$Items)
    if (-not $Items -or $Items.Count -eq 0) { return '' }
    $p = $Items[0]
    for ($i = 1; $i -lt $Items.Count; $i++) {
        $s = $Items[$i]
        $m = [Math]::Min($p.Length, $s.Length)
        $j = 0
        while ($j -lt $m -and $p[$j] -eq $s[$j]) { $j++ }
        $p = $p.Substring(0, $j)
        if ($p.Length -eq 0) { break }
    }
    return $p
}

function Write-CompletionList {
    
    param([string[]]$Items)
    if (-not $Items -or $Items.Count -eq 0) { return }
    $w = [Math]::Max(20, [Console]::WindowWidth)
    $maxlen = 0
    foreach ($it in $Items) { if ($it.Length -gt $maxlen) { $maxlen = $it.Length } }
    $cell = $maxlen + 2
    $cols = [Math]::Max(1, [Math]::Floor(($w - 4) / $cell))
    $i = 0
    while ($i -lt $Items.Count) {
        $sb = [System.Text.StringBuilder]::new()
        [void]$sb.Append('    ')
        for ($c = 0; $c -lt $cols -and $i -lt $Items.Count; $c++) {
            [void]$sb.Append($Items[$i].PadRight($cell))
            $i++
        }
        Write-Host ($sb.ToString().TrimEnd()) -ForegroundColor DarkGray
    }
}

function Get-PathCompletions {
    
    param([string]$Token)
    $sep = [Math]::Max($Token.LastIndexOf('\'), $Token.LastIndexOf('/'))
    if ($sep -ge 0) {
        $keep = $Token.Substring(0, $sep + 1)
        $leaf = $Token.Substring($sep + 1)
        $searchDir = $keep
    } else {
        $keep = ''
        $leaf = $Token
        $searchDir = '.'
    }
    $out = [System.Collections.Generic.List[string]]::new()
    try {
        $items = Get-ChildItem -LiteralPath $searchDir -ErrorAction Stop
    } catch {
        return @()
    }
    foreach ($it in $items) {
        if ($it.Name -like "$leaf*") {
            $rep = $keep + $it.Name
            if ($it.PSIsContainer) { $rep += '\' }
            $out.Add($rep)
        }
    }
    return @($out | Sort-Object)
}

function Get-TraktCompletions {
    param([string]$Line)

    try {
        $segments = @(Split-TraktCommandChain -Line $Line -AllowIncomplete)
        if ($segments.Count -gt 0) { $Line = $segments[-1].Text }
        $tokens = @(Split-TraktTokens -Line $Line -AllowIncomplete)
    } catch { return @() }

    $endsSpace = $Line -match '\s$'
    $prefix = if ($tokens.Count -gt 0 -and -not $endsSpace) { $tokens[-1] } else { '' }
    $index = if ($endsSpace) { $tokens.Count } else { [math]::Max(0, $tokens.Count - 1) }

    if ($index -eq 0) {
        $names = @($global:TraktCommands.Keys) + @($global:Aliases.Keys)
        if ($global:UserAliases) { $names += @($global:UserAliases.Keys) }
        $names += @('clear', 'exit', 'repeat')
        return @($names | Where-Object { $_ -like "$prefix*" } | Sort-Object -Unique)
    }

    $command = $tokens[0].ToLowerInvariant()
    if ($global:Aliases.ContainsKey($command)) { $command = $global:Aliases[$command] }

    if ($prefix -match '[\\/]' -or $prefix.StartsWith('.')) {
        return @(Get-PathCompletions $prefix)
    }
    if ($global:TraktCommands.Contains($command)) {
        return @($global:TraktCommands[$command].Options | Where-Object { $_ -like "$prefix*" })
    }
    return @()
}

function Format-TraktLineColored {
    param([string]$Buffer, [string[]]$Known)
    if (-not $Buffer) { return '' }
    $sp = $Buffer.IndexOf(' ')
    $first = if ($sp -ge 0) { $Buffer.Substring(0, $sp) } else { $Buffer }
    $rest = if ($sp -ge 0) { $Buffer.Substring($sp) } else { '' }
    $ok = $Known -contains $first.ToLower()
    $col = if ($ok) { "`e[38;2;120;200;140m" } else { "`e[38;2;240;120;120m" }
    return $col + $first + "`e[0m" + $rest
}


function Read-TraktReverseSearch {
    
    param(
        [System.Collections.Generic.List[string]]$History,
        [int]$OriginLeft,
        [int]$OriginTop
    )
    $out = [Console]::Out
    $query = ''
    $matchIdx = -1

    $find = {
        param([int]$from)
        if (-not $query) {
            if ($History.Count -gt 0) { return $History.Count - 1 }
            return -1
        }
        for ($i = $from; $i -ge 0; $i--) {
            if ($History[$i].IndexOf($query, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) { return $i }
        }
        return -1
    }
    $matchIdx = & $find ($History.Count - 1)

    $render = {
        $match = if ($matchIdx -ge 0) { $History[$matchIdx] } else { '' }
        $failTag = if ($query -and $matchIdx -lt 0) { 'failing ' } else { '' }
        $line = "(${failTag}reverse-i-search)'$query': $match"
        $w = [Math]::Max(1, [Console]::WindowWidth)
        $room = [Math]::Max(1, $w - $OriginLeft - 1)
        if ($line.Length -gt $room) { $line = $line.Substring(0, $room) }
        $top = [Math]::Min($OriginTop, [Console]::BufferHeight - 1)
        [Console]::SetCursorPosition($OriginLeft, $top)
        $out.Write($line + "`e[0K`e[0J")
    }
    & $render

    while ($true) {
        $k = [Console]::ReadKey($true)
        $ctrl = ($k.Modifiers -band [ConsoleModifiers]::Control) -ne 0

        if ($k.Key -eq [ConsoleKey]::Enter) {
            $m = if ($matchIdx -ge 0) { $History[$matchIdx] } else { $query }
            return @{ Action = 'execute'; Text = $m }
        }
        if ($k.Key -eq [ConsoleKey]::Escape) {
            return @{ Action = 'cancel'; Text = '' }
        }
        if ($ctrl -and $k.Key -eq [ConsoleKey]::R) {
            $next = & $find ($matchIdx - 1)
            if ($next -ge 0) { $matchIdx = $next }
            & $render
            continue
        }
        if ($k.Key -eq [ConsoleKey]::Backspace) {
            if ($query.Length -gt 0) {
                $query = $query.Substring(0, $query.Length - 1)
                $matchIdx = & $find ($History.Count - 1)
                & $render
            }
            continue
        }
        if ($k.Key -in @([ConsoleKey]::LeftArrow, [ConsoleKey]::RightArrow, [ConsoleKey]::Home,
                          [ConsoleKey]::End, [ConsoleKey]::UpArrow, [ConsoleKey]::DownArrow)) {
            $m = if ($matchIdx -ge 0) { $History[$matchIdx] } else { '' }
            return @{ Action = 'edit'; Text = $m }
        }
        $ch = $k.KeyChar
        if ($ch -and -not [char]::IsControl($ch)) {
            $query += $ch
            $matchIdx = & $find ($History.Count - 1)
            & $render
        }
    }
}


function Get-TraktPaletteCandidates {
    $items = [System.Collections.Generic.List[object]]::new()
    foreach ($name in $global:TraktCommands.Keys) {
        $items.Add(@{ Text = $name; Hint = $global:TraktCommands[$name].Summary })
    }
    foreach ($line in @('audit', 'test tools', 'ci audit tools', 'status', 'trace last', 'mux')) {
        $items.Add(@{ Text = $line; Hint = 'quick action' })
    }
    $history = Get-TraktHistory
    $seen = @{}
    $added = 0
    for ($i = $history.Count - 1; $i -ge 0 -and $added -lt 40; $i--) {
        $line = $history[$i]
        if (-not $seen.ContainsKey($line)) {
            $seen[$line] = $true
            $items.Add(@{ Text = $line; Hint = 'history' })
            $added++
        }
    }
    return $items.ToArray()
}

function Invoke-TraktPalette {
    
    $cands = Get-TraktPaletteCandidates
    $query = ''
    $sel = 0
    $out = [Console]::Out
    $out.Write("`e[?1049h`e[?25l")
    try {
        while ($true) {
            $w = [math]::Max(20, [Console]::WindowWidth)
            $h = [math]::Max(8, [Console]::WindowHeight)
            $maxRows = $h - 5

            $scored = foreach ($c in $cands) {
                $m = Get-FuzzyMatch -Pattern $query -Text $c.Text
                if ($null -ne $m) { [pscustomobject]@{ C = $c; S = $m.Score; I = $m.Indices } }
            }
            $list = @($scored | Sort-Object S -Descending | Select-Object -First $maxRows)
            if ($sel -ge $list.Count) { $sel = [math]::Max(0, $list.Count - 1) }

            $sb = [System.Text.StringBuilder]::new(8192)
            [void]$sb.Append("`e[H`e[2J`e[0m")
            [void]$sb.Append("`e[1;36m  trakt palette`e[0m")
            [void]$sb.Append("`e[2m   type to filter, enter run, esc cancel`e[0m`n")
            [void]$sb.Append("  > ").Append($query).Append("`e[7m `e[0m`n`n")

            for ($row = 0; $row -lt $list.Count; $row++) {
                $it = $list[$row]
                $text = $it.C.Text
                $hint = [string]$it.C.Hint
                $idxSet = [System.Collections.Generic.HashSet[int]]::new()
                foreach ($ix in $it.I) { [void]$idxSet.Add($ix) }
                $isSel = ($row -eq $sel)
                if ($isSel) { [void]$sb.Append("`e[48;2;45;48;60m") }
                [void]$sb.Append('  ')
                $limit = [math]::Min($text.Length, 40)
                for ($ci = 0; $ci -lt $limit; $ci++) {
                    if ($idxSet.Contains($ci)) {
                        [void]$sb.Append("`e[1;36m").Append($text[$ci]).Append("`e[22;39m")
                    } else {
                        [void]$sb.Append($text[$ci])
                    }
                }
                if ($text.Length -lt 42) { [void]$sb.Append(' ' * (42 - $limit)) }
                if ($hint) {
                    $hlimit = [math]::Max(0, $w - 46)
                    if ($hint.Length -gt $hlimit) { $hint = $hint.Substring(0, $hlimit) }
                    [void]$sb.Append("`e[2m").Append($hint).Append("`e[22m")
                }
                [void]$sb.Append("`e[0m`n")
            }
            if ($list.Count -eq 0) { [void]$sb.Append("`e[2m  (no matches)`e[0m`n") }
            $out.Write($sb.ToString())

            $k = [Console]::ReadKey($true)
            if ($k.Key -eq [ConsoleKey]::Escape) { return $null }
            elseif ($k.Key -eq [ConsoleKey]::Enter) {
                if ($list.Count -gt 0) { return $list[$sel].C.Text }
                return $null
            }
            elseif ($k.Key -eq [ConsoleKey]::UpArrow) { if ($sel -gt 0) { $sel-- } }
            elseif ($k.Key -eq [ConsoleKey]::DownArrow) { if ($sel -lt ($list.Count - 1)) { $sel++ } }
            elseif ($k.Key -eq [ConsoleKey]::Backspace) {
                if ($query.Length -gt 0) { $query = $query.Substring(0, $query.Length - 1); $sel = 0 }
            }
            else {
                $ch = $k.KeyChar
                if ($ch -and -not [char]::IsControl($ch)) { $query += $ch; $sel = 0 }
            }
        }
    }
    finally {
        $out.Write("`e[?25h`e[?1049l")
    }
}


function Read-TraktLine {
    
    param([scriptblock]$RenderPrompt)

    if ([Console]::IsInputRedirected) { return (Read-Host) }

    $st = @{
        Buffer = ''
        Cursor = 0
        Left   = [Console]::CursorLeft
        Top    = [Console]::CursorTop
    }
    $known = @(Get-TraktCommandNames)
    if ($global:Aliases) { $known += @($global:Aliases.Keys) }
    if ($global:UserAliases) { $known += @($global:UserAliases.Keys) }
    $known += @('clear', 'cls', 'exit', 'repeat')

    $hist = Get-TraktHistory
    $histIdx = $hist.Count
    $savedEdit = ''
    $tabState = $null

    $redraw = {
        param($st, $known)
        $w = [math]::Max(1, [Console]::WindowWidth)
        $bh = [Console]::BufferHeight
        $rows = [math]::Floor(($st.Left + $st.Buffer.Length) / $w)
        if (($st.Top + $rows) -ge $bh) { $st.Top = [math]::Max(0, $bh - 1 - $rows) }
        [Console]::SetCursorPosition($st.Left, $st.Top)
        [Console]::Out.Write((Format-TraktLineColored -Buffer $st.Buffer -Known $known) + "`e[0m`e[0J")
        $cpos = $st.Left + $st.Cursor
        $cy = [math]::Min($bh - 1, $st.Top + [math]::Floor($cpos / $w))
        [Console]::SetCursorPosition(($cpos % $w), $cy)
    }

    $prevCC = [Console]::TreatControlCAsInput
    [Console]::TreatControlCAsInput = $true
    try {
        while ($true) {
            & $redraw $st $known
            $k = [Console]::ReadKey($true)
            $ctrl = ($k.Modifiers -band [ConsoleModifiers]::Control) -ne 0
            $alt = ($k.Modifiers -band [ConsoleModifiers]::Alt) -ne 0
            if ($k.Key -ne [ConsoleKey]::Tab) { $tabState = $null }

            if ($k.Key -eq [ConsoleKey]::Enter) {
                [Console]::Out.Write("`n")
                Add-TraktHistory $st.Buffer
                return $st.Buffer
            }
            elseif ($ctrl -and $k.Key -eq [ConsoleKey]::C) {
                [Console]::Out.Write("^C`n")
                return ''
            }
            elseif ($ctrl -and $k.Key -eq [ConsoleKey]::D) {
                if ($st.Buffer.Length -eq 0) {
                    [Console]::Out.Write("`n")
                    return 'exit'
                }
                if ($st.Cursor -lt $st.Buffer.Length) {
                    $st.Buffer = $st.Buffer.Remove($st.Cursor, 1)
                }
            }
            elseif ($ctrl -and $k.Key -eq [ConsoleKey]::R) {
                $r = Read-TraktReverseSearch -History $hist -OriginLeft $st.Left -OriginTop $st.Top
                if ($r.Action -eq 'execute') {
                    $st.Buffer = $r.Text
                    $st.Cursor = $st.Buffer.Length
                    & $redraw $st $known
                    [Console]::Out.Write("`n")
                    Add-TraktHistory $st.Buffer
                    return $st.Buffer
                }
                elseif ($r.Action -eq 'edit') {
                    $st.Buffer = $r.Text
                    $st.Cursor = $st.Buffer.Length
                }
            }
            elseif ($ctrl -and $k.Key -eq [ConsoleKey]::L) {
                [Console]::Clear()
                if ($RenderPrompt) { & $RenderPrompt }
                $st.Left = [Console]::CursorLeft
                $st.Top  = [Console]::CursorTop
            }
            elseif ($ctrl -and $k.Key -eq [ConsoleKey]::P) {
                $selText = Invoke-TraktPalette
                if ($selText) {
                    $st.Buffer = $selText
                    $st.Cursor = $selText.Length
                    & $redraw $st $known
                    [Console]::Out.Write("`n")
                    Add-TraktHistory $selText
                    return $selText
                }
            }
            elseif ($ctrl -and $k.Key -eq [ConsoleKey]::A) { $st.Cursor = 0 }
            elseif ($ctrl -and $k.Key -eq [ConsoleKey]::E) { $st.Cursor = $st.Buffer.Length }
            elseif ($ctrl -and $k.Key -eq [ConsoleKey]::U) {
                $st.Buffer = $st.Buffer.Substring($st.Cursor)
                $st.Cursor = 0
            }
            elseif ($ctrl -and $k.Key -eq [ConsoleKey]::K) {
                $st.Buffer = $st.Buffer.Substring(0, $st.Cursor)
            }
            elseif ($ctrl -and $k.Key -eq [ConsoleKey]::W) {
                if ($st.Cursor -gt 0) {
                    $i = Get-PrevWordStart $st.Buffer $st.Cursor
                    $st.Buffer = $st.Buffer.Remove($i, $st.Cursor - $i)
                    $st.Cursor = $i
                }
            }
            elseif ($alt -and $k.Key -eq [ConsoleKey]::B) {
                $st.Cursor = Get-PrevWordStart $st.Buffer $st.Cursor
            }
            elseif ($alt -and $k.Key -eq [ConsoleKey]::F) {
                $st.Cursor = Get-NextWordEnd $st.Buffer $st.Cursor
            }
            elseif ($k.Key -eq [ConsoleKey]::Backspace) {
                if ($st.Cursor -gt 0) {
                    $st.Buffer = $st.Buffer.Remove($st.Cursor - 1, 1)
                    $st.Cursor--
                }
            }
            elseif ($k.Key -eq [ConsoleKey]::Delete) {
                if ($st.Cursor -lt $st.Buffer.Length) {
                    $st.Buffer = $st.Buffer.Remove($st.Cursor, 1)
                }
            }
            elseif ($k.Key -eq [ConsoleKey]::LeftArrow) {
                if ($ctrl) { $st.Cursor = Get-PrevWordStart $st.Buffer $st.Cursor }
                elseif ($st.Cursor -gt 0) { $st.Cursor-- }
            }
            elseif ($k.Key -eq [ConsoleKey]::RightArrow) {
                if ($ctrl) { $st.Cursor = Get-NextWordEnd $st.Buffer $st.Cursor }
                elseif ($st.Cursor -lt $st.Buffer.Length) { $st.Cursor++ }
            }
            elseif ($k.Key -eq [ConsoleKey]::Home) { $st.Cursor = 0 }
            elseif ($k.Key -eq [ConsoleKey]::End) { $st.Cursor = $st.Buffer.Length }
            elseif ($k.Key -eq [ConsoleKey]::Escape) {
                $st.Buffer = ''
                $st.Cursor = 0
            }
            elseif ($k.Key -eq [ConsoleKey]::UpArrow) {
                if ($histIdx -gt 0) {
                    if ($histIdx -eq $hist.Count) { $savedEdit = $st.Buffer }
                    $histIdx--
                    $st.Buffer = $hist[$histIdx]
                    $st.Cursor = $st.Buffer.Length
                }
            }
            elseif ($k.Key -eq [ConsoleKey]::DownArrow) {
                if ($histIdx -lt $hist.Count) {
                    $histIdx++
                    if ($histIdx -eq $hist.Count) { $st.Buffer = $savedEdit }
                    else { $st.Buffer = $hist[$histIdx] }
                    $st.Cursor = $st.Buffer.Length
                }
            }
            elseif ($k.Key -eq [ConsoleKey]::Tab) {
                if ($st.Cursor -ne $st.Buffer.Length) { continue }
                if ($null -eq $tabState) {
                    $start = $st.Buffer.LastIndexOf(' ') + 1
                    $cands = @(Get-TraktCompletions -Line $st.Buffer)
                    $tabState = @{ Start = $start; Cands = $cands; Listed = $false; Idx = -1 }
                }
                $cands = $tabState.Cands
                $start = $tabState.Start
                if ($cands.Count -eq 0) { continue }
                elseif ($cands.Count -eq 1) {
                    $st.Buffer = $st.Buffer.Substring(0, $start) + $cands[0]
                    $st.Cursor = $st.Buffer.Length
                    $tabState = $null
                }
                else {
                    $token = $st.Buffer.Substring($start)
                    $lcp = Get-CommonPrefix $cands
                    if ($lcp.Length -gt $token.Length) {
                        $st.Buffer = $st.Buffer.Substring(0, $start) + $lcp
                        $st.Cursor = $st.Buffer.Length
                        $tabState = $null
                    }
                    elseif (-not $tabState.Listed) {
                        [Console]::Out.Write("`n")
                        Write-CompletionList $cands
                        if ($RenderPrompt) { & $RenderPrompt } else { [Console]::Out.Write('> ') }
                        $st.Left = [Console]::CursorLeft
                        $st.Top  = [Console]::CursorTop
                        $tabState.Listed = $true
                    }
                    else {
                        $back = ($k.Modifiers -band [ConsoleModifiers]::Shift) -ne 0
                        $idx = $tabState.Idx
                        if ($back) { $idx-- } else { $idx++ }
                        if ($idx -ge $cands.Count) { $idx = 0 }
                        if ($idx -lt 0) { $idx = $cands.Count - 1 }
                        $st.Buffer = $st.Buffer.Substring(0, $start) + $cands[$idx]
                        $st.Cursor = $st.Buffer.Length
                        $tabState.Idx = $idx
                    }
                }
            }
            else {
                $ch = $k.KeyChar
                if (-not $alt -and $ch -and -not [char]::IsControl($ch)) {
                    $st.Buffer = $st.Buffer.Insert($st.Cursor, $ch)
                    $st.Cursor++
                    $histIdx = $hist.Count
                }
            }
        }
    }
    finally {
        [Console]::TreatControlCAsInput = $prevCC
    }
}
