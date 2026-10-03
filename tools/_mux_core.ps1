# Pane model, layout, input dispatch, and rendering.
#Requires -Version 7.3


class TraktMuxScreen {
    [int]$Width
    [int]$Height
    [object]$Native

    TraktMuxScreen([int]$w, [int]$h) {
        $t = [type]'TraktScreen'
        $this.Native = [Activator]::CreateInstance($t, @([object]$w, [object]$h))
        $this.Width = $this.Native.Width
        $this.Height = $this.Native.Height
    }

    static [int] Rgb([int]$r, [int]$g, [int]$b) {
        return ((($r -band 0xFF) -shl 16) -bor (($g -band 0xFF) -shl 8) -bor ($b -band 0xFF))
    }

    [void] Resize([int]$w, [int]$h) {
        $this.Native.Resize($w, $h)
        $this.Width = $this.Native.Width
        $this.Height = $this.Native.Height
    }

    [void] Clear([int]$background) {
        $this.Native.Clear($background)
    }

    [void] Set([int]$x, [int]$y, [char]$c, [int]$f, [int]$b) {
        $this.Native.Set($x, $y, $c, $f, $b)
    }

    [void] Text([int]$x, [int]$y, [string]$s, [int]$f, [int]$b) {
        $this.Native.Text($x, $y, $s, $f, $b, -1)
    }

    [void] TextMax([int]$x, [int]$y, [string]$s, [int]$f, [int]$b, [int]$max) {
        $this.Native.Text($x, $y, $s, $f, $b, $max)
    }

    [void] FillRect([int]$x, [int]$y, [int]$w, [int]$h, [char]$c, [int]$f, [int]$b) {
        $this.Native.FillRect($x, $y, $w, $h, $c, $f, $b)
    }

    [void] Box([int]$x, [int]$y, [int]$w, [int]$h, [int]$f, [int]$b, [string]$style) {
        $round = 1
        if ($style -eq 'single') { $round = 0 }
        $this.Native.Box($x, $y, $w, $h, $f, $b, $round)
    }

    [void] VLine([int]$x, [int]$y, [int]$h, [char]$c, [int]$f, [int]$b) {
        $this.Native.VLine($x, $y, $h, $c, $f, $b)
    }

    [void] HLine([int]$x, [int]$y, [int]$w, [char]$c, [int]$f, [int]$b) {
        $this.Native.HLine($x, $y, $w, $c, $f, $b)
    }

    [void] Flush() {
        $this.Native.Flush()
    }
}


class TraktTheme {
    static [int] $Bg          = [TraktMuxScreen]::Rgb(24, 24, 28)
    static [int] $Panel       = [TraktMuxScreen]::Rgb(32, 33, 40)
    static [int] $Border      = [TraktMuxScreen]::Rgb(70, 72, 84)
    static [int] $BorderFocus = [TraktMuxScreen]::Rgb(90, 170, 255)
    static [int] $Text        = [TraktMuxScreen]::Rgb(210, 212, 220)
    static [int] $TextDim     = [TraktMuxScreen]::Rgb(130, 133, 145)
    static [int] $TextHead    = [TraktMuxScreen]::Rgb(240, 242, 248)
    static [int] $Accent      = [TraktMuxScreen]::Rgb(120, 200, 140)
    static [int] $StatusBg    = [TraktMuxScreen]::Rgb(45, 48, 60)
    static [int] $StatusFg    = [TraktMuxScreen]::Rgb(200, 203, 214)
    static [int] $StatusHi    = [TraktMuxScreen]::Rgb(250, 220, 120)
}


class TraktMuxPane {
    [string]$Title = 'pane'
    [object]$Engine = $null
    [object]$Node = $null

    [void] Render([TraktMuxScreen]$s, [int]$x, [int]$y, [int]$w, [int]$h, [bool]$focused) { }
    [bool] OnKey([object]$ev) { return $false }
    [bool] OnMouse([object]$ev, [int]$lx, [int]$ly) { return $false }
    [void] OnTick() { }
}


class TraktClockPane : TraktMuxPane {
    [datetime]$Start

    TraktClockPane() {
        $this.Title = 'clock'
        $this.Start = Get-Date
    }

    [void] Render([TraktMuxScreen]$s, [int]$x, [int]$y, [int]$w, [int]$h, [bool]$focused) {
        $now = Get-Date
        $t = $now.ToString('HH:mm:ss')
        $d = $now.ToString('yyyy-MM-dd dddd', [System.Globalization.CultureInfo]::InvariantCulture)
        $up = (Get-Date) - $this.Start
        $upStr = 'up ' + [int]$up.TotalMinutes + 'm ' + $up.Seconds + 's'
        $cy = $y + [int]($h / 2) - 1
        $s.TextMax($x + [math]::Max(0, [int](($w - $t.Length) / 2)), $cy, $t, [TraktTheme]::Accent, [TraktTheme]::Panel, $w)
        $s.TextMax($x + [math]::Max(0, [int](($w - $d.Length) / 2)), $cy + 1, $d, [TraktTheme]::Text, [TraktTheme]::Panel, $w)
        $s.TextMax($x + [math]::Max(0, [int](($w - $upStr.Length) / 2)), $cy + 2, $upStr, [TraktTheme]::TextDim, [TraktTheme]::Panel, $w)
    }
}


class TraktSysInfoPane : TraktMuxPane {
    [string[]]$Lines
    [bool]$Loaded = $false

    TraktSysInfoPane() {
        $this.Title = 'system'
        $this.Lines = @('(loading system information)')
    }

    [void] OnTick() {
        if ($this.Loaded) { return }
        $this.Loaded = $true
        $acc = [System.Collections.Generic.List[string]]::new()
        try {
            $acc.Add('OS   : ' + [System.Environment]::OSVersion.VersionString)
            $cpu = (Get-CimInstance Win32_Processor -ErrorAction SilentlyContinue | Select-Object -First 1).Name
            $acc.Add('CPU  : ' + $cpu)
            $ram = [math]::Round((Get-CimInstance Win32_ComputerSystem -ErrorAction SilentlyContinue).TotalPhysicalMemory / 1GB, 1)
            $acc.Add('RAM  : ' + $ram + ' GB')
            $acc.Add('PS   : ' + $global:PSVersionTable.PSVersion.ToString())
            $acc.Add('PID  : ' + [System.Diagnostics.Process]::GetCurrentProcess().Id)
        } catch {
            $acc.Add('error gathering system information')
        }
        $this.Lines = $acc.ToArray()
    }

    [void] Render([TraktMuxScreen]$s, [int]$x, [int]$y, [int]$w, [int]$h, [bool]$focused) {
        for ($i = 0; $i -lt $this.Lines.Count; $i++) {
            if ($i -ge $h) { break }
            $s.TextMax($x, $y + $i, $this.Lines[$i], [TraktTheme]::Text, [TraktTheme]::Panel, $w)
        }
    }
}


class TraktHelpPane : TraktMuxPane {
    TraktHelpPane() { $this.Title = 'help' }

    [void] Render([TraktMuxScreen]$s, [int]$x, [int]$y, [int]$w, [int]$h, [bool]$focused) {
        $lines = @(
            'Keyboard',
            '  Ctrl+Arrows   move focus by direction',
            '  Tab / Sh+Tab  cycle focus',
            '  Ctrl+S / D    split side by side / stacked',
            '  Ctrl+W        close pane   Ctrl+Z zoom',
            '  Ctrl+Q        quit',
            '  Esc           close overlay / unzoom',
            '',
            'Mouse',
            '  click focus, double click zoom, drag',
            '  dividers, wheel scrolls, status buttons',
            '',
            'New panes open a menu; pick content there.',
            'Layout is saved on quit and restored.'
        )
        for ($i = 0; $i -lt $lines.Count; $i++) {
            if ($i -ge $h) { break }
            $ln = $lines[$i]
            $fg = if ($ln -match '^\S' -and $ln -notmatch '^\s') { [TraktTheme]::TextHead } else { [TraktTheme]::TextDim }
            $s.TextMax($x, $y + $i, $ln, $fg, [TraktTheme]::Panel, $w)
        }
    }
}


class TraktLogPane : TraktMuxPane {
    TraktLogPane() { $this.Title = 'log' }

    [void] Render([TraktMuxScreen]$s, [int]$x, [int]$y, [int]$w, [int]$h, [bool]$focused) {
        if ($null -eq $this.Engine) {
            $s.TextMax($x, $y, '(no engine)', [TraktTheme]::TextDim, [TraktTheme]::Panel, $w)
            return
        }
        $log = $this.Engine.Log
        $count = $log.Count
        if ($count -eq 0) {
            $s.TextMax($x, $y, '(log empty)', [TraktTheme]::TextDim, [TraktTheme]::Panel, $w)
            return
        }
        $begin = [math]::Max(0, $count - $h)
        for ($i = 0; $i -lt $h; $i++) {
            $idx = $begin + $i
            if ($idx -ge $count) { break }
            $s.TextMax($x, $y + $i, $log[$idx], [TraktTheme]::TextDim, [TraktTheme]::Panel, $w)
        }
    }
}


class TraktMenuPane : TraktMuxPane {
    [int]$Sel = 0

    TraktMenuPane() { $this.Title = 'menu' }

    [string[]] Names() {
        if ($this.Engine -and $this.Engine.Registry) {
            return @($this.Engine.Registry.Keys)
        }
        return @()
    }

    [void] Render([TraktMuxScreen]$s, [int]$x, [int]$y, [int]$w, [int]$h, [bool]$focused) {
        $names = $this.Names()
        $s.TextMax($x, $y, 'Select content:', [TraktTheme]::TextHead, [TraktTheme]::Panel, $w)
        if ($names.Count -eq 0) {
            $s.TextMax($x, $y + 2, '(no registry configured)', [TraktTheme]::TextDim, [TraktTheme]::Panel, $w)
            return
        }
        if ($this.Sel -ge $names.Count) { $this.Sel = $names.Count - 1 }
        for ($i = 0; $i -lt $names.Count; $i++) {
            $ry = $y + 2 + $i
            if ($ry -ge $y + $h) { break }
            $mark = if ($i -eq $this.Sel) { '> ' } else { '  ' }
            $fg = if ($i -eq $this.Sel) { [TraktTheme]::Accent } else { [TraktTheme]::Text }
            $s.TextMax($x, $ry, $mark + $names[$i], $fg, [TraktTheme]::Panel, $w)
        }
    }

    [bool] OnKey([object]$ev) {
        if ($ev.Ctrl -or -not $ev.KeyDown) { return $false }
        $names = $this.Names()
        switch ($ev.VKey) {
            0x26 { if ($this.Sel -gt 0) { $this.Sel-- }; return $true }
            0x28 { if ($this.Sel -lt $names.Count - 1) { $this.Sel++ }; return $true }
            0x0D { $this.Activate(); return $true }
        }
        return $false
    }

    [bool] OnMouse([object]$ev, [int]$lx, [int]$ly) {
        if ($ev.Left) {
            $i = $ly - 2
            if ($i -ge 0 -and $i -lt $this.Names().Count) {
                $this.Sel = $i
                $this.Activate()
            }
            return $true
        }
        return $false
    }

    [void] Activate() {
        if ($null -eq $this.Engine -or $null -eq $this.Engine.Registry) { return }
        $names = $this.Names()
        if ($this.Sel -lt 0 -or $this.Sel -ge $names.Count) { return }
        $entry = $this.Engine.Registry[$names[$this.Sel]]
        $pane = & ($entry['Make'])
        if ($null -ne $pane -and $null -ne $this.Engine) {
            $this.Engine.SwapContent($this.Node, $pane)
        }
    }
}


class TraktMuxEngine {
    [TraktMuxScreen]$Screen
    $Root
    [string]$FocusId
    [System.Collections.Generic.List[object]]$PaneRects
    [System.Collections.Generic.List[object]]$DivRects
    [System.Collections.Generic.List[object]]$Buttons
    [System.Collections.Generic.List[string]]$Log
    [bool]$Running = $false
    [bool]$Zoom = $false
    [bool]$PrevLeft = $false
    $Drag = $null
    [int]$PaneCounter = 100

    [System.Collections.Specialized.OrderedDictionary]$Registry

    [object]$Overlay = $null
    $OverlayRect = $null

    [string]$LastInput = ''

    TraktMuxEngine() {
        $this.PaneRects = [System.Collections.Generic.List[object]]::new()
        $this.DivRects = [System.Collections.Generic.List[object]]::new()
        $this.Buttons = [System.Collections.Generic.List[object]]::new()
        $this.Log = [System.Collections.Generic.List[string]]::new()
        $this.Registry = [System.Collections.Specialized.OrderedDictionary]::new()
    }


    [void] LogMsg([string]$m) {
        $ts = (Get-Date).ToString('HH:mm:ss')
        $this.Log.Add("[$ts] $m")
        while ($this.Log.Count -gt 300) { $this.Log.RemoveAt(0) }
    }


    [object] FindById($node, [string]$id) {
        if ($node['Kind'] -eq 'pane') {
            if ($node['Id'] -eq $id) { return $node }
            return $null
        }
        $a = $this.FindById($node['A'], $id)
        if ($a) { return $a }
        return $this.FindById($node['B'], $id)
    }

    [object] FocusedNode() {
        return $this.FindById($this.Root, $this.FocusId)
    }

    [object] FindParent($cur, $target) {
        if ($cur['Kind'] -ne 'split') { return $null }
        if ([object]::ReferenceEquals($cur['A'], $target) -or [object]::ReferenceEquals($cur['B'], $target)) {
            return $cur
        }
        $p = $this.FindParent($cur['A'], $target)
        if ($p) { return $p }
        return $this.FindParent($cur['B'], $target)
    }

    [bool] ReplaceNode($cur, $target, $replacement) {
        if ($cur['Kind'] -ne 'split') { return $false }
        if ([object]::ReferenceEquals($cur['A'], $target)) { $cur['A'] = $replacement; return $true }
        if ([object]::ReferenceEquals($cur['B'], $target)) { $cur['B'] = $replacement; return $true }
        if ($this.ReplaceNode($cur['A'], $target, $replacement)) { return $true }
        return $this.ReplaceNode($cur['B'], $target, $replacement)
    }

    [string] FirstPaneId($node) {
        if ($node['Kind'] -eq 'pane') { return $node['Id'] }
        return $this.FirstPaneId($node['A'])
    }

    [void] AttachPanes() { $this.AttachWalk($this.Root) }
    [void] AttachWalk($node) {
        if ($node['Kind'] -eq 'pane') {
            $p = [TraktMuxPane]$node['Pane']
            $p.Engine = $this
            $p.Node = $node
            return
        }
        $this.AttachWalk($node['A'])
        $this.AttachWalk($node['B'])
    }

    [void] TickAll() { $this.TickWalk($this.Root) }
    [void] TickWalk($node) {
        if ($node['Kind'] -eq 'pane') {
            ([TraktMuxPane]$node['Pane']).OnTick()
            return
        }
        $this.TickWalk($node['A'])
        $this.TickWalk($node['B'])
    }

    [void] SwapContent($node, [TraktMuxPane]$newPane) {
        $node['Pane'] = $newPane
        $newPane.Engine = $this
        $newPane.Node = $node
        $this.LogMsg("content set to '$($newPane.Title)'")
    }


    [void] ComputeLayout() {
        $this.PaneRects.Clear()
        $this.DivRects.Clear()
        $bottom = $this.Screen.Height - 1
        if ($bottom -lt 1) { $bottom = 1 }
        if ($this.Zoom) {
            $fn = $this.FocusedNode()
            if ($fn) {
                $this.PaneRects.Add(@{ Node = $fn; X = 0; Y = 0; W = $this.Screen.Width; H = $bottom })
                return
            }
        }
        $this.ComputeNode($this.Root, 0, 0, $this.Screen.Width, $bottom)
    }

    [void] ComputeNode($node, [int]$x, [int]$y, [int]$w, [int]$h) {
        $node['X'] = $x; $node['Y'] = $y; $node['W'] = $w; $node['H'] = $h
        if ($node['Kind'] -eq 'pane') {
            $this.PaneRects.Add(@{ Node = $node; X = $x; Y = $y; W = $w; H = $h })
            return
        }
        if ($node['Dir'] -eq 'h') {
            $avail = $w - 1
            if ($avail -lt 2) { $this.ComputeNode($node['A'], $x, $y, $w, $h); return }
            $leftW = [int][math]::Floor($avail * $node['Ratio'])
            if ($leftW -lt 1) { $leftW = 1 }
            if ($leftW -gt $avail - 1) { $leftW = $avail - 1 }
            $rightW = $avail - $leftW
            $divX = $x + $leftW
            $this.ComputeNode($node['A'], $x, $y, $leftW, $h)
            $this.DivRects.Add(@{ Node = $node; X = $divX; Y = $y; W = 1; H = $h; Dir = 'h' })
            $this.ComputeNode($node['B'], $divX + 1, $y, $rightW, $h)
        } else {
            $avail = $h - 1
            if ($avail -lt 2) { $this.ComputeNode($node['A'], $x, $y, $w, $h); return }
            $topH = [int][math]::Floor($avail * $node['Ratio'])
            if ($topH -lt 1) { $topH = 1 }
            if ($topH -gt $avail - 1) { $topH = $avail - 1 }
            $botH = $avail - $topH
            $divY = $y + $topH
            $this.ComputeNode($node['A'], $x, $y, $w, $topH)
            $this.DivRects.Add(@{ Node = $node; X = $x; Y = $divY; W = $w; H = 1; Dir = 'v' })
            $this.ComputeNode($node['B'], $x, $divY + 1, $w, $botH)
        }
    }

    [object] HitPane([int]$x, [int]$y) {
        foreach ($p in $this.PaneRects) {
            if ($x -ge $p['X'] -and $x -lt ($p['X'] + $p['W']) -and $y -ge $p['Y'] -and $y -lt ($p['Y'] + $p['H'])) {
                return $p
            }
        }
        return $null
    }


    [void] MoveFocus([string]$dir) {
        $this.Zoom = $false
        $this.ComputeLayout()
        $cur = $this.FocusedNode()
        if (-not $cur) { return }
        $crect = $null
        foreach ($p in $this.PaneRects) {
            if ([object]::ReferenceEquals($p['Node'], $cur)) { $crect = $p; break }
        }
        if (-not $crect) { return }
        $ccx = $crect['X'] + $crect['W'] / 2.0
        $ccy = $crect['Y'] + $crect['H'] / 2.0
        $best = $null
        $bestd = [double]::MaxValue
        foreach ($p in $this.PaneRects) {
            if ([object]::ReferenceEquals($p['Node'], $cur)) { continue }
            $pcx = $p['X'] + $p['W'] / 2.0
            $pcy = $p['Y'] + $p['H'] / 2.0
            $dx = $pcx - $ccx
            $dy = $pcy - $ccy
            $ok = $false
            switch ($dir) {
                'left'  { $ok = ($dx -lt -0.5 -and [math]::Abs($dy) -le [math]::Abs($dx)) }
                'right' { $ok = ($dx -gt 0.5 -and [math]::Abs($dy) -le [math]::Abs($dx)) }
                'up'    { $ok = ($dy -lt -0.5 -and [math]::Abs($dx) -le [math]::Abs($dy)) }
                'down'  { $ok = ($dy -gt 0.5 -and [math]::Abs($dx) -le [math]::Abs($dy)) }
            }
            if (-not $ok) { continue }
            $d = $dx * $dx + $dy * $dy
            if ($d -lt $bestd) { $bestd = $d; $best = $p['Node'] }
        }
        if ($best) { $this.FocusId = $best['Id']; $this.LogMsg("focus $dir -> $($best['Id'])") }
    }

    [void] CycleFocus([int]$delta) {
        $this.Zoom = $false
        $this.ComputeLayout()
        if ($this.PaneRects.Count -eq 0) { return }
        $ids = @()
        foreach ($p in $this.PaneRects) { $ids += $p['Node']['Id'] }
        $idx = [array]::IndexOf($ids, $this.FocusId)
        if ($idx -lt 0) { $idx = 0 }
        $idx = ($idx + $delta + $ids.Count) % $ids.Count
        $this.FocusId = $ids[$idx]
    }

    [void] SplitFocused([string]$dir) {
        $node = $this.FocusedNode()
        if (-not $node) { return }
        $this.PaneCounter++
        $newId = "p$($this.PaneCounter)"
        $newNode = @{ Kind = 'pane'; Id = $newId; Pane = [TraktMenuPane]::new() }
        $split = @{ Kind = 'split'; Dir = $dir; Ratio = 0.5; A = $node; B = $newNode }
        if ([object]::ReferenceEquals($this.Root, $node)) {
            $this.Root = $split
        } else {
            $this.ReplaceNode($this.Root, $node, $split)
        }
        $this.AttachPanes()
        $this.FocusId = $newId
        $this.Zoom = $false
        $label = if ($dir -eq 'h') { 'side by side' } else { 'top and bottom' }
        $this.LogMsg("split $label -> $newId")
    }

    [void] CloseFocused() {
        $node = $this.FocusedNode()
        if (-not $node) { return }
        if ([object]::ReferenceEquals($this.Root, $node)) {
            $this.LogMsg('cannot close the last pane')
            return
        }
        $par = $this.FindParent($this.Root, $node)
        if (-not $par) { return }
        $sibling = if ([object]::ReferenceEquals($par['A'], $node)) { $par['B'] } else { $par['A'] }
        if ([object]::ReferenceEquals($this.Root, $par)) {
            $this.Root = $sibling
        } else {
            $this.ReplaceNode($this.Root, $par, $sibling)
        }
        $this.AttachPanes()
        $this.FocusId = $this.FirstPaneId($sibling)
        $this.Zoom = $false
        $this.LogMsg('closed pane')
    }

    [void] DoAction([string]$a) {
        switch ($a) {
            'splitH' { $this.SplitFocused('h') }
            'splitV' { $this.SplitFocused('v') }
            'zoom'   { $this.Zoom = -not $this.Zoom }
            'close'  { $this.CloseFocused() }
            'quit'   { $this.Running = $false }
        }
    }


    [void] ShowOverlay([object]$pane) {
        $pane.Engine = $this
        $this.Overlay = $pane
    }

    [void] CloseOverlay() {
        $this.Overlay = $null
        $this.OverlayRect = $null
    }

    [string] DescribeKey([object]$ev) {
        $mods = ''
        if ($ev.Ctrl) { $mods += 'Ctrl+' }
        if ($ev.Alt) { $mods += 'Alt+' }
        if ($ev.Shift) { $mods += 'Shift+' }
        $vk = $ev.VKey
        $name = switch ($vk) {
            0x08 { 'Bksp' } 0x09 { 'Tab' } 0x0D { 'Enter' } 0x1B { 'Esc' }
            0x20 { 'Space' } 0x21 { 'PgUp' } 0x22 { 'PgDn' } 0x23 { 'End' }
            0x24 { 'Home' } 0x25 { 'Left' } 0x26 { 'Up' } 0x27 { 'Right' }
            0x28 { 'Down' } 0x2E { 'Del' }
            default {
                if ($vk -ge 0x30 -and $vk -le 0x5A) { [string][char]$vk }
                elseif ($vk -ge 0x70 -and $vk -le 0x7B) { 'F' + ($vk - 0x6F) }
                else { 'vk' + $vk }
            }
        }
        return $mods + $name
    }


    [void] HandleKey([object]$ev) {
        if (-not $ev.KeyDown) { return }
        $vk = $ev.VKey
        $this.LastInput = 'key ' + $this.DescribeKey($ev)

        if ($ev.Ctrl -and $vk -eq 0x51) { $this.Running = $false; return }

        if ($this.Overlay) {
            if ($vk -eq 0x1B) { $this.CloseOverlay(); return }
            $null = $this.Overlay.OnKey($ev)
            return
        }

        $fn = $this.FocusedNode()
        if ($fn) {
            $pane = [TraktMuxPane]$fn['Pane']
            if ($pane.OnKey($ev)) { return }
        }

        if ($ev.Ctrl) {
            switch ($vk) {
                0x25 { $this.MoveFocus('left'); return }
                0x27 { $this.MoveFocus('right'); return }
                0x26 { $this.MoveFocus('up'); return }
                0x28 { $this.MoveFocus('down'); return }
                0x53 { $this.SplitFocused('h'); return }   # S
                0x44 { $this.SplitFocused('v'); return }   # D
                0x57 { $this.CloseFocused(); return }      # W
                0x5A { $this.Zoom = -not $this.Zoom; return } # Z
            }
        }
        if ($vk -eq 0x09) {
            if ($ev.Shift) { $this.CycleFocus(-1) } else { $this.CycleFocus(1) }
            return
        }
        if ($vk -eq 0x1B) {
            if ($this.Zoom) { $this.Zoom = $false }
            return
        }
    }

    [void] HandleMouse([object]$ev) {
        $x = $ev.X
        $y = $ev.Y
        $leftDown = $ev.Left
        $pressEdge = ($leftDown -and -not $this.PrevLeft)
        $releaseEdge = (-not $leftDown -and $this.PrevLeft)

        if ($pressEdge) { $this.LastInput = 'click ' + $x + ',' + $y }
        elseif ($ev.Wheel -ne 0) { $this.LastInput = 'wheel' }

        if ($this.Overlay) {
            $orct = $this.OverlayRect
            $handled = $true
            if ($null -ne $orct) {
                $inside = ($x -ge $orct['X'] -and $x -lt ($orct['X'] + $orct['W']) -and
                           $y -ge $orct['Y'] -and $y -lt ($orct['Y'] + $orct['H']))
                if ($ev.Wheel -ne 0) {
                    $null = $this.Overlay.OnMouse($ev, $x - $orct['X'] - 1, $y - $orct['Y'] - 1)
                } elseif ($pressEdge) {
                    if ($inside) {
                        $null = $this.Overlay.OnMouse($ev, $x - $orct['X'] - 1, $y - $orct['Y'] - 1)
                    } else {
                        $this.CloseOverlay()
                        $handled = $false
                    }
                }
            }
            if ($handled) {
                $this.PrevLeft = $leftDown
                return
            }
        }

        if ($ev.Wheel -ne 0) {
            $hit = $this.HitPane($x, $y)
            if ($hit) {
                ([TraktMuxPane]$hit['Node']['Pane']).OnMouse($ev, $x - $hit['X'] - 1, $y - $hit['Y'] - 1)
            }
            $this.PrevLeft = $leftDown
            return
        }

        if ($pressEdge) {
            foreach ($b in $this.Buttons) {
                if ($y -eq $b['Y'] -and $x -ge $b['X'] -and $x -lt ($b['X'] + $b['W'])) {
                    $this.DoAction($b['Action'])
                    $this.PrevLeft = $leftDown
                    return
                }
            }
            foreach ($d in $this.DivRects) {
                if ($x -ge $d['X'] -and $x -lt ($d['X'] + $d['W']) -and $y -ge $d['Y'] -and $y -lt ($d['Y'] + $d['H'])) {
                    $this.Drag = $d
                    $this.PrevLeft = $leftDown
                    return
                }
            }
            $hit = $this.HitPane($x, $y)
            if ($hit) {
                $this.FocusId = $hit['Node']['Id']
                if ($ev.DoubleClick) {
                    $this.Zoom = -not $this.Zoom
                } else {
                    ([TraktMuxPane]$hit['Node']['Pane']).OnMouse($ev, $x - $hit['X'] - 1, $y - $hit['Y'] - 1)
                }
            }
            $this.PrevLeft = $leftDown
            return
        }

        if ($leftDown -and $ev.Moved -and $this.Drag) {
            $node = $this.Drag['Node']
            if ($node['Dir'] -eq 'h') {
                $avail = $node['W'] - 1
                if ($avail -ge 2) {
                    $r = ($x - $node['X']) / [double]$avail
                    $node['Ratio'] = [math]::Max(0.1, [math]::Min(0.9, $r))
                }
            } else {
                $avail = $node['H'] - 1
                if ($avail -ge 2) {
                    $r = ($y - $node['Y']) / [double]$avail
                    $node['Ratio'] = [math]::Max(0.1, [math]::Min(0.9, $r))
                }
            }
            $this.PrevLeft = $leftDown
            return
        }

        if ($releaseEdge) { $this.Drag = $null }
        $this.PrevLeft = $leftDown
    }


    [void] Render() {
        $s = $this.Screen
        if (-not $this.FocusedNode()) { $this.FocusId = $this.FirstPaneId($this.Root) }
        $s.Clear([TraktTheme]::Bg)
        $this.ComputeLayout()

        foreach ($pr in $this.PaneRects) {
            $node = $pr['Node']
            $focused = ($node['Id'] -eq $this.FocusId)
            $pane = [TraktMuxPane]$node['Pane']
            $bx = $pr['X']; $by = $pr['Y']; $bw = $pr['W']; $bh = $pr['H']
            $s.FillRect($bx, $by, $bw, $bh, [char]' ', [TraktTheme]::Text, [TraktTheme]::Panel)
            $bc = if ($focused) { [TraktTheme]::BorderFocus } else { [TraktTheme]::Border }
            $s.Box($bx, $by, $bw, $bh, $bc, [TraktTheme]::Panel, 'round')
            $title = if ($focused) { ' * ' + $pane.Title + ' ' } else { ' ' + $pane.Title + ' ' }
            $s.TextMax($bx + 2, $by, $title, $bc, [TraktTheme]::Panel, [math]::Max(0, $bw - 4))
            $ix = $bx + 1; $iy = $by + 1; $iw = $bw - 2; $ih = $bh - 2
            if ($iw -gt 0 -and $ih -gt 0) {
                $pane.Render($s, $ix, $iy, $iw, $ih, $focused)
            }
        }

        foreach ($d in $this.DivRects) {
            $hovered = ($this.Drag -and [object]::ReferenceEquals($this.Drag['Node'], $d['Node']))
            $col = if ($hovered) { [TraktTheme]::Accent } else { [TraktTheme]::Border }
            if ($d['Dir'] -eq 'h') {
                $s.VLine($d['X'], $d['Y'], $d['H'], [char]'|', $col, [TraktTheme]::Bg)
                $midy = $d['Y'] + [int]($d['H'] / 2)
                $s.Set($d['X'], $midy, [char]'|', $col, [TraktTheme]::Bg)
            } else {
                $s.HLine($d['X'], $d['Y'], $d['W'], [char]'-', $col, [TraktTheme]::Bg)
                $midx = $d['X'] + [int]($d['W'] / 2)
                $s.Set($midx, $d['Y'], [char]'=', $col, [TraktTheme]::Bg)
            }
        }

        $this.RenderStatus()

        if ($this.Overlay) {
            $ow = [int]($s.Width * 0.74)
            if ($ow -lt 44) { $ow = [math]::Min($s.Width - 2, 44) }
            $oh = [int]($s.Height * 0.74)
            if ($oh -lt 10) { $oh = [math]::Min($s.Height - 2, 10) }
            $ox = [int](($s.Width - $ow) / 2)
            $oy = [int](($s.Height - $oh) / 2)
            $s.FillRect($ox, $oy, $ow, $oh, [char]' ', [TraktTheme]::Text, [TraktTheme]::Panel)
            $s.Box($ox, $oy, $ow, $oh, [TraktTheme]::BorderFocus, [TraktTheme]::Panel, 'round')
            $otitle = ' ' + ([TraktMuxPane]$this.Overlay).Title + '  (Esc closes) '
            $s.TextMax($ox + 2, $oy, $otitle, [TraktTheme]::BorderFocus, [TraktTheme]::Panel, [math]::Max(0, $ow - 4))
            try {
                ([TraktMuxPane]$this.Overlay).Render($s, $ox + 1, $oy + 1, $ow - 2, $oh - 2, $true)
            } catch {
                $s.TextMax($ox + 2, $oy + 2, ('overlay render error: ' + $_.Exception.Message), [TraktMuxScreen]::Rgb(240, 100, 100), [TraktTheme]::Panel, [math]::Max(0, $ow - 4))
            }
            $this.OverlayRect = @{ X = $ox; Y = $oy; W = $ow; H = $oh }
        } else {
            $this.OverlayRect = $null
        }

        $s.Flush()
    }

    [void] RenderStatus() {
        $s = $this.Screen
        $y = $s.Height - 1
        if ($y -lt 0) { return }
        $s.FillRect(0, $y, $s.Width, 1, [char]' ', [TraktTheme]::StatusFg, [TraktTheme]::StatusBg)
        $fn = $this.FocusedNode()
        $ft = if ($fn) { ([TraktMuxPane]$fn['Pane']).Title } else { '-' }
        $mode = if ($this.Zoom) { '[ZOOM] ' } else { '' }
        $s.TextMax(1, $y, 'trakt-mux', [TraktTheme]::StatusHi, [TraktTheme]::StatusBg, $s.Width)
        $left = $mode + 'focus: ' + $ft
        if ($this.LastInput) { $left += '   in: ' + $this.LastInput }
        if ($this.Overlay) { $left = '[overlay open, Esc closes]   ' + $left }
        $s.TextMax(12, $y, $left, [TraktTheme]::StatusFg, [TraktTheme]::StatusBg, [math]::Max(0, $s.Width - 12))

        $this.Buttons.Clear()
        $defs = @(
            @{ L = ' [ | ] '; A = 'splitH' },
            @{ L = ' [ - ] '; A = 'splitV' },
            @{ L = ' [ z ] '; A = 'zoom' },
            @{ L = ' [ x ] '; A = 'close' },
            @{ L = ' [ q ] '; A = 'quit' }
        )
        $curx = $s.Width - 1
        for ($k = $defs.Count - 1; $k -ge 0; $k--) {
            $lbl = $defs[$k]['L']
            $wid = $lbl.Length
            $startx = $curx - $wid + 1
            if ($startx -lt 0) { break }
            $s.Text($startx, $y, $lbl, [TraktTheme]::StatusHi, [TraktTheme]::StatusBg)
            $this.Buttons.Add(@{ X = $startx; Y = $y; W = $wid; Action = $defs[$k]['A'] })
            $curx = $startx - 1
        }
    }

    [void] Run() {
        $consoleType = [type]'TraktConsole'
        $consoleType::Setup()
        $previousControlC = [Console]::TreatControlCAsInput
        $output = [Console]::Out
        try {
            [Console]::TreatControlCAsInput = $true
            $output.Write("`e[?1049h`e[?25l")
            $output.Flush()
            $size = $consoleType::GetSize()
            $this.Screen = [TraktMuxScreen]::new($size[0], $size[1])
            $this.AttachPanes()
            $this.Running = $true
            while ($this.Running) {
                $size = $consoleType::GetSize()
                if ($size[0] -ne $this.Screen.Width -or $size[1] -ne $this.Screen.Height) {
                    $this.Screen.Resize($size[0], $size[1])
                }
                $this.ComputeLayout()
                foreach ($eventItem in $consoleType::Poll()) {
                    if ($eventItem.Kind -eq 'key') { $this.HandleKey($eventItem) }
                    elseif ($eventItem.Kind -eq 'mouse') { $this.HandleMouse($eventItem) }
                }
                $this.TickAll()
                $this.Render()
                Start-Sleep -Milliseconds 33
            }
        } finally {
            try {
                $output.Write("`e[0m`e[?25h`e[?1049l")
                $output.Flush()
            } finally {
                [Console]::TreatControlCAsInput = $previousControlC
                $consoleType::Restore()
            }
        }
    }
}