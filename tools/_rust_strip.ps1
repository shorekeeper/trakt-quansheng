# Best-effort Rust API digest. Its output is not build input.
#Requires -Version 7.3

class TraktRustStripper {
    [string]$Src
    [int]$Pos
    [System.Text.StringBuilder]$Out
    [int]$DataElideThreshold   # lines; const/static bodies longer than this get elided. 0 = never.

    TraktRustStripper([string]$src) {
        $this.Src = $src
        $this.Pos = 0
        $this.Out = [System.Text.StringBuilder]::new($src.Length)
        $this.DataElideThreshold = 30
    }

    [bool] Eof() { return $this.Pos -ge $this.Src.Length }
    [char] Cur() { return $this.Src[$this.Pos] }
    [char] Peek([int]$n) {
        $i = $this.Pos + $n
        if ($i -lt $this.Src.Length) { return $this.Src[$i] }
        return [char]0
    }

    [char] ConsumeAtom([bool]$emit) {
        $c = $this.Cur()

        if ($c -eq '/' -and $this.Peek(1) -eq '/') {
            while (-not $this.Eof() -and $this.Cur() -ne "`n") {
                if ($emit) { $null = $this.Out.Append($this.Cur()) }
                $this.Pos++
            }
            return [char]0
        }
        if ($c -eq '/' -and $this.Peek(1) -eq '*') {
            $depth = 0
            while (-not $this.Eof()) {
                if ($this.Cur() -eq '/' -and $this.Peek(1) -eq '*') {
                    $depth++
                    if ($emit) { $null = $this.Out.Append('/*') }
                    $this.Pos += 2; continue
                }
                if ($this.Cur() -eq '*' -and $this.Peek(1) -eq '/') {
                    $depth--
                    if ($emit) { $null = $this.Out.Append('*/') }
                    $this.Pos += 2
                    if ($depth -eq 0) { break } else { continue }
                }
                if ($emit) { $null = $this.Out.Append($this.Cur()) }
                $this.Pos++
            }
            return [char]0
        }

        $prevIdent = $false
        if ($this.Pos -gt 0) {
            $p = $this.Src[$this.Pos - 1]
            $prevIdent = [char]::IsLetterOrDigit($p) -or $p -eq '_'
        }

        if (-not $prevIdent -and ($c -eq 'r' -or ($c -eq 'b' -and $this.Peek(1) -eq 'r'))) {
            $start = $this.Pos
            $i = $this.Pos + $(if ($c -eq 'b') { 2 } else { 1 })
            $hashes = 0
            while ($i -lt $this.Src.Length -and $this.Src[$i] -eq '#') { $hashes++; $i++ }
            if ($i -lt $this.Src.Length -and $this.Src[$i] -eq '"') {
                $i++
                while ($i -lt $this.Src.Length) {
                    if ($this.Src[$i] -eq '"') {
                        $ok = $true
                        for ($h = 1; $h -le $hashes; $h++) {
                            if (($i + $h) -ge $this.Src.Length -or $this.Src[$i + $h] -ne '#') { $ok = $false; break }
                        }
                        if ($ok) { $i += 1 + $hashes; break }
                    }
                    $i++
                }
                if ($emit) { $null = $this.Out.Append($this.Src.Substring($start, $i - $start)) }
                $this.Pos = $i
                return [char]0
            }
        }

        if ($c -eq '"' -or ($c -eq 'b' -and $this.Peek(1) -eq '"' -and -not $prevIdent)) {
            if ($c -eq 'b') {
                if ($emit) { $null = $this.Out.Append('b') }
                $this.Pos++
            }
            if ($emit) { $null = $this.Out.Append('"') }
            $this.Pos++
            while (-not $this.Eof()) {
                $s = $this.Cur()
                if ($s -eq '\') {
                    if ($emit) { $null = $this.Out.Append($s).Append($this.Peek(1)) }
                    $this.Pos += 2; continue
                }
                if ($emit) { $null = $this.Out.Append($s) }
                $this.Pos++
                if ($s -eq '"') { break }
            }
            return [char]0
        }

        if ($c -eq "'") {
            $isChar = ($this.Peek(1) -eq '\') -or ($this.Peek(2) -eq "'")
            if ($isChar) {
                $end = $this.Pos + 1
                if ($this.Src[$end] -eq '\') { $end++ }
                $end++
                if ($end -lt $this.Src.Length -and $this.Src[$end] -eq "'") { $end++ }
                if ($emit) { $null = $this.Out.Append($this.Src.Substring($this.Pos, $end - $this.Pos)) }
                $this.Pos = $end
                return [char]0
            }
        }

        if ($emit) { $null = $this.Out.Append($c) }
        $this.Pos++
        return $c
    }

    [void] Balanced([bool]$emit) {
        $depth = 0
        while (-not $this.Eof()) {
            $ch = $this.ConsumeAtom($emit)
            if ($ch -eq '{') { $depth++ }
            elseif ($ch -eq '}') {
                $depth--
                if ($depth -eq 0) { return }
            }
        }
    }

    [int] MeasureBalancedLines() {
        $savePos = $this.Pos
        $depth = 0
        $lines = 0
        while (-not $this.Eof()) {
            if ($this.Cur() -eq "`n") { $lines++ }
            $ch = $this.ConsumeAtom($false)
            if ($ch -eq '{') { $depth++ }
            elseif ($ch -eq '}') {
                $depth--
                if ($depth -eq 0) { break }
            }
        }
        $result = $lines
        $this.Pos = $savePos
        return $result
    }

    [void] Scope([bool]$stopAtBrace) {
        $word = ""
        $kw = ""          # first significant keyword of current item; sticky until item ends
        $lastIdent = ""
        $sigDepth = 0     # () and [] nesting within the current signature

        while (-not $this.Eof()) {
            $ch = $this.ConsumeAtom($true)
            if ($ch -eq [char]0) { continue }

            if ([char]::IsLetterOrDigit($ch) -or $ch -eq '_') {
                $word += $ch
                continue
            }

            if ($word) {
                if (-not $kw) {
                    switch ($word) {
                        { $_ -in 'fn','macro_rules' }          { $kw = 'fn' }
                        { $_ -in 'struct','enum','union' }     { $kw = 'data' }
                        { $_ -in 'impl','trait' }              { $kw = 'container' }
                        { $_ -in 'const','static' }            { $kw = 'constdata' }
                        'mod'                                  { $kw = 'mod' }
                    }
                }
                $lastIdent = $word
                $word = ""
            }

            if ($ch -eq '(' -or $ch -eq '[') { $sigDepth++; continue }
            if ($ch -eq ')' -or $ch -eq ']') {
                if ($sigDepth -gt 0) { $sigDepth-- }
                continue
            }

            if ($ch -eq ';') {
                if ($sigDepth -eq 0) { $kw = ""; $lastIdent = ""; $sigDepth = 0 }
                continue
            }

            if ($ch -eq '{') {
                switch ($kw) {
                    'fn' {
                        $null = $this.Out.Remove($this.Out.Length - 1, 1)
                        $this.Pos--
                        $this.Balanced($false)
                        $null = $this.Out.Append(';')
                    }
                    'data' {
                        $this.Pos--
                        $null = $this.Out.Remove($this.Out.Length - 1, 1)
                        $this.Balanced($true)
                    }
                    'constdata' {
                        $this.Pos--
                        $null = $this.Out.Remove($this.Out.Length - 1, 1)
                        if ($this.DataElideThreshold -gt 0) {
                            $lines = $this.MeasureBalancedLines()
                            if ($lines -gt $this.DataElideThreshold) {
                                $this.Balanced($false)
                                $null = $this.Out.Append("{ /* ~$lines lines of data elided */ }")
                            } else {
                                $this.Balanced($true)
                            }
                        } else {
                            $this.Balanced($true)
                        }
                    }
                    'mod' {
                        if ($lastIdent -eq 'tests') {
                            $null = $this.Out.Remove($this.Out.Length - 1, 1)
                            $this.Pos--
                            $this.Balanced($false)
                            $null = $this.Out.Append('{ /* tests stripped */ }')
                        } else {
                            $this.Scope($true)
                        }
                    }
                    'container' { $this.Scope($true) }
                    default {
                        $this.Pos--
                        $null = $this.Out.Remove($this.Out.Length - 1, 1)
                        $this.Balanced($true)
                    }
                }
                $kw = ""; $lastIdent = ""; $sigDepth = 0
                continue
            }

            if ($ch -eq '}' -and $stopAtBrace) { return }
        }
    }

    [string] Strip() {
        $this.Scope($false)
        return $this.Out.ToString()
    }
}

function Convert-RustToStub {
    param(
        [string]$Path,
        [int]$DataElideThreshold = 30
    )
    $src = [System.IO.File]::ReadAllText($Path)
    $stripper = [TraktRustStripper]::new($src)
    $stripper.DataElideThreshold = $DataElideThreshold
    return $stripper.Strip()
}
