#Requires -Version 7.3
# One foreground display session. No jobs, timers, modules, or device I/O.
# Progress counts planned stages, not compiler work or estimated time.

$global:TraktBuildUi = $null

function Get-TraktFirmwareUiSteps {
    param([string]$Variant)

    $names = if ($Variant -ceq 'all') {
        @(Get-TraktVariantNames)
    } else {
        @((Get-TraktVariant $Variant).Name)
    }

    # One reservation, then five stages for each dev/release build.
    return 1 + $names.Count * 10
}

function Get-TraktBuildStepStatus {
    param([string]$Status, [int]$Warnings)

    if ($Status -ceq 'PASS' -and $Warnings -gt 0) { return 'WARN' }
    return $Status
}

function Get-TraktBuildBar {
    param([int]$Done, [int]$Total, [long]$Milliseconds)

    if ($Total -lt 1 -or $Done -lt 0 -or $Done -gt $Total) {
        throw 'Invalid build progress counters.'
    }

    $width = 18
    $filled = [int][math]::Floor($width * [double]$Done / $Total)
    $remaining = $width - $filled
    if ($remaining -eq 0) { return '#' * $width }

    [char[]]$cells = (('#' * $filled) + ('-' * $remaining)).ToCharArray()
    $length = [math]::Min(3, $remaining)
    $travel = $remaining - $length
    $position = 0

    if ($travel -gt 0) {
        $position = [int]([math]::Floor($Milliseconds / 140) % (2 * $travel))
        if ($position -gt $travel) { $position = 2 * $travel - $position }
    }

    for ($index = 0; $index -lt $length; $index++) {
        $cells[$filled + $position + $index] = '='
    }

    return -join $cells
}

function Limit-TraktBuildLiveLine {
    param([string]$Text, [int]$Columns)

    if ($Columns -lt 1) { return '' }

    # Single-cell ASCII keeps transient width accounting deterministic.
    # Complete diagnostics retain their original text outside this line.
    $line = [regex]::Replace(
        (ConvertTo-TraktBuildLine $Text), '[^\x20-\x7E]', '?'
    )
    if ($line.Length -le $Columns) { return $line }
    if ($Columns -le 3) { return $line.Substring(0, $Columns) }

    return $line.Substring(0, $Columns - 3) + '...'
}

function Clear-TraktBuildLiveLine {
    $ui = $global:TraktBuildUi
    if ($null -eq $ui -or -not $ui.Painted) { return }

    try {
        # Reset attributes before erasing so no colored background remains.
        [Console]::Out.Write("`r`e[0m`e[2K")
        [Console]::Out.Flush()
    } catch {
        $ui.Live = $false
    } finally {
        $ui.Painted = $false
        $ui.NextPaint = 0L
    }
}

function ConvertTo-TraktBuildLine {
    param([AllowEmptyString()][string]$Text)

    # Terminal controls cannot become part of the transient status line.
    return ([regex]::Replace($Text, '[\x00-\x1F\x7F]', ' ')).Trim()
}

function Start-TraktBuildUi {
    param([string]$Label, [int]$Steps)

    if ($null -ne $global:TraktBuildUi) { return $false }
    if ($Steps -lt 1) { throw 'A build display requires at least one stage.' }

    $live = $false
    $glyphs = $false
    try {
        $live = $Host.Name -eq 'ConsoleHost' -and
            $Host.UI.SupportsVirtualTerminal -and
            -not [Console]::IsOutputRedirected -and
            -not [Console]::IsInputRedirected -and
            $env:TERM -ne 'dumb' -and
            [string]$PSStyle.OutputRendering -ne 'PlainText' -and
            $ProgressPreference -ne 'SilentlyContinue' -and
            [Console]::WindowWidth -ge 40
        $glyphs = $live -and [Console]::OutputEncoding.CodePage -eq 65001
    } catch {
        $live = $false
    }

    $global:TraktBuildUi = [pscustomobject]@{
        Label = $Label
        Total = $Steps
        Done = 0
        Live = $live
        Glyphs = $glyphs
        Clock = [System.Diagnostics.Stopwatch]::StartNew()
        NextPaint = 0L
        Painted = $false
        Step = $null
        Stages = [System.Collections.Generic.List[object]]::new()
        Images = [System.Collections.Generic.List[string]]::new()
    }

    Write-Host ''
    Write-Host "TRAKT $Label | $Steps stages" -ForegroundColor Cyan
    return $true
}

function Update-TraktBuildUi {
    $ui = $global:TraktBuildUi
    if ($null -eq $ui -or -not $ui.Live -or $null -eq $ui.Step) { return }

    $elapsed = $ui.Clock.ElapsedMilliseconds
    if ($elapsed -lt $ui.NextPaint) { return }
    $ui.NextPaint = $elapsed + 100

    try {
        # Leave two columns unused to avoid writing at the wrapping edge.
        $columns = [int][math]::Min(144, [Console]::WindowWidth - 2)
        if ($columns -lt 38) {
            Clear-TraktBuildLiveLine
            return
        }

        $step = $ui.Step
        $bar = Get-TraktBuildBar $ui.Done $ui.Total $elapsed
        $percent = [int][math]::Floor(100 * $ui.Done / $ui.Total)
        $status = '{0,3}% [{1}] {2}/{3}  {4}  {5}' -f
            $percent, $bar, $ui.Done, $ui.Total, $step.Name,
            (Format-Duration $step.Clock.Elapsed.TotalMilliseconds)

        if ($step.Warnings + $step.Errors -gt 0) {
            $status += "  W:$($step.Warnings) E:$($step.Errors)"
        }

        # File-by-file audit activity remains internal. Cargo's actual
        # messages remain useful when enough horizontal space is available.
        if ($step.Name -match ' Cargo ' -and $step.Detail -and
            $columns - $status.Length -ge 24) {
            $status += ' | ' + $step.Detail
        }

        $status = Limit-TraktBuildLiveLine -Text $status -Columns $columns
        $ui.Painted = $true
        [Console]::Out.Write("`r`e[0m`e[2K`e[36m$status`e[0m`r")
        [Console]::Out.Flush()
    } catch {
        Clear-TraktBuildLiveLine
        $ui.Live = $false
    }
}

function Set-TraktBuildDetail {
    param([string]$Text)

    $ui = $global:TraktBuildUi
    if ($null -eq $ui -or $null -eq $ui.Step) { return }
    $ui.Step.Detail = ConvertTo-TraktBuildLine $Text
    Update-TraktBuildUi
}

function Enter-TraktBuildStep {
    param([string]$Name, [string]$Detail = '')

    $ui = $global:TraktBuildUi
    if ($null -eq $ui) { return }
    if ($null -ne $ui.Step) {
        throw "Unfinished build stage: $($ui.Step.Name)"
    }

    $ui.Step = [pscustomobject]@{
        Name = $Name
        Detail = $Detail
        Clock = [System.Diagnostics.Stopwatch]::StartNew()
        Warnings = 0
        Errors = 0
        Built = 0
        Fresh = 0
    }
    $ui.NextPaint = 0L

    if (-not $ui.Live) {
        Write-Host ("  [RUN] {0}" -f $Name)
    }
    Update-TraktBuildUi
}

function Exit-TraktBuildStep {
    param(
        [ValidateSet('PASS', 'WARN', 'FAIL')][string]$Status = 'PASS',
        [string]$Detail = ''
    )

    $ui = $global:TraktBuildUi
    if ($null -eq $ui -or $null -eq $ui.Step) { return }

    $step = $ui.Step
    $step.Clock.Stop()
    if (-not $Detail) { $Detail = $step.Detail }
    $Detail = ConvertTo-TraktBuildLine $Detail
    $statusText = Get-TraktBuildStepStatus $Status $step.Warnings

    $ui.Stages.Add([pscustomobject]@{
        Name = $step.Name
        Status = $statusText
        DurationMs = [math]::Round($step.Clock.Elapsed.TotalMilliseconds, 3)
        Detail = $Detail
        Warnings = $step.Warnings
        Errors = $step.Errors
        BuiltArtifacts = $step.Built
        FreshArtifacts = $step.Fresh
    })

    $ui.Done++
    $ui.Step = $null

    Clear-TraktBuildLiveLine

    $mark = switch ($statusText) {
        PASS { if ($ui.Glyphs) { [string][char]0x2713 } else { 'OK' } }
        WARN { if ($ui.Glyphs) { ([string][char]0x2713) + '!' } else { 'WARN' } }
        FAIL { if ($ui.Glyphs) { [string][char]0x2717 } else { 'FAIL' } }
    }
    $color = switch ($statusText) {
        PASS { 'Green' }
        WARN { 'Yellow' }
        FAIL { 'Red' }
    }

    $line = '  [{0}] {1,-30} {2,8}' -f
        $mark, $step.Name, (Format-Duration $step.Clock.Elapsed.TotalMilliseconds)
    if ($Detail) { $line += ' | ' + $Detail }
    if ($step.Built + $step.Fresh -gt 0) {
        $line += " | built=$($step.Built) cached=$($step.Fresh)"
    }
    if ($step.Warnings + $step.Errors -gt 0) {
        $line += " | W:$($step.Warnings) E:$($step.Errors)"
    }

    if ($ui.Live) { Write-Host $line -ForegroundColor $color }
    else { Write-Host $line }
}

function ConvertFrom-TraktCargoEvent {
    param(
        [string]$Line,
        [ValidateSet('stdout', 'stderr')][string]$Stream
    )

    $kind = 'Status'
    $warningCount = 0
    $errorCount = 0
    $text = ''
    $fresh = $false

    if ($Stream -ceq 'stdout') {
        if (-not $Line.StartsWith('{')) { return $null }
        try {
            $message = ConvertFrom-Json -InputObject $Line -AsHashtable -ErrorAction Stop
        } catch {
            # The authoritative Cargo decoder still rejects malformed output.
            return $null
        }

        switch ($message.reason) {
            'compiler-artifact' {
                $kind = 'Artifact'
                $fresh = $message.fresh -eq $true
                $state = if ($fresh) { 'cached' } else { 'produced' }
                $text = '{0} [{1}] {2}' -f
                    $message.target.name, ($message.target.kind -join ','), $state
            }
            'compiler-message' {
                $diagnostic = $message.message
                $warningCount = [int]($diagnostic.level -ceq 'warning')
                $errorCount = [int]($diagnostic.level -in 'error', 'error: internal compiler error')
                $kind = 'Diagnostic'
                $text = '{0}: {1}' -f $diagnostic.level, $diagnostic.message

                $primary = @($diagnostic.spans | Where-Object {
                    $_.is_primary
                } | Select-Object -First 1)
                if ($primary.Count -eq 1) {
                    $text += ' ({0}:{1}:{2})' -f
                        $primary[0].file_name, $primary[0].line_start,
                        $primary[0].column_start
                }
            }
            'build-finished' {
                $text = if ($message.success) {
                    'Cargo reported successful completion'
                } else {
                    'Cargo reported failed completion'
                }
            }
            'build-script-executed' {
                $text = 'Unexpected build script; output validation will reject it'
            }
            default { return $null }
        }
    } else {
        $text = $Line.Trim()
        if (-not $text) { return $null }

        if ($text -match '^warning:') {
            $kind = 'Diagnostic'
            $warningCount = 1
        } elseif ($text -match '^error:') {
            $kind = 'Diagnostic'
            $errorCount = 1
        } elseif ($text -notmatch '^(Compiling|Checking|Fresh|Finished|Blocking|Updating|Downloading)\b') {
            return $null
        }
    }

    return [pscustomobject]@{
        Kind = $kind
        Text = ConvertTo-TraktBuildLine $text
        Warnings = $warningCount
        Errors = $errorCount
        Fresh = $fresh
    }
}

function Receive-TraktCargoEvent {
    param([string]$Line, [string]$Stream)

    $ui = $global:TraktBuildUi
    if ($null -eq $ui -or $null -eq $ui.Step) { return }

    $event = ConvertFrom-TraktCargoEvent -Line $Line -Stream $Stream
    if ($null -eq $event) { return }

    $step = $ui.Step
    $step.Detail = $event.Text
    $step.Warnings += $event.Warnings
    $step.Errors += $event.Errors

    if ($event.Kind -ceq 'Artifact') {
        if ($event.Fresh) { $step.Fresh++ } else { $step.Built++ }
    }

    if ($event.Warnings + $event.Errors -gt 0) {
        Clear-TraktBuildLiveLine
        $color = if ($event.Errors) { 'Red' } else { 'Yellow' }
        if ($ui.Live) { Write-Host ('    ' + $event.Text) -ForegroundColor $color }
        else { Write-Host ('    ' + $event.Text) }
    }

    Update-TraktBuildUi
}

function Add-TraktBuildImage {
    param([string]$Profile, [string]$Variant, [object]$Elf, [string]$Directory)

    $ui = $global:TraktBuildUi
    if ($null -eq $ui) { return }

    $m = $Elf.Memory
    $ui.Images.Add(('{0,-10} {1,-7} Flash {2,6} B  RAM {3,5} B  free Flash {4,6} B' -f
        $Variant, $Profile, $m.FlashLoadBytes, $m.StaticRamBytes,
        $m.FlashRemainingBytes))
    $ui.Images.Add("  $Directory")
}

function Finish-TraktBuildUi {
    param([bool]$Owner, [object]$Result)

    $ui = $global:TraktBuildUi
    if ($null -eq $ui) { return }

    if ($null -ne $ui.Step) {
        $detail = if ($null -eq $Result) {
            'Interrupted'
        } else {
            [string](@($Result.Output | Where-Object { $_ } | Select-Object -First 1) -join '')
        }
        Exit-TraktBuildStep -Status FAIL -Detail $detail
    }

    if (-not $Owner) { return }

    try {
        $ui.Clock.Stop()
        Clear-TraktBuildLiveLine

        if ($null -ne $Result) {
            $Result | Add-Member -NotePropertyName BuildStages `
                -NotePropertyValue $ui.Stages.ToArray() -Force

            # Raw Output remains authoritative for traces and other callers.
            # Interactive entries use DisplayOutput to avoid printing it twice.
            if ($Result.ExitCode -eq 0) {
                $display = [System.Collections.Generic.List[string]]::new()
                if ($ui.Images.Count -gt 0) {
                    $display.Add('')
                    $display.Add('Validated artifacts:')
                    foreach ($line in $ui.Images) { $display.Add($line) }
                }
                $warnings = 0
                foreach ($stage in $ui.Stages) { $warnings += $stage.Warnings }
                $display.Add(('{0}: {1}; stages {2}/{3}; warnings {4}; elapsed {5}' -f
                    $ui.Label, $Result.Status, $ui.Done, $ui.Total, $warnings,
                    (Format-Duration $ui.Clock.Elapsed.TotalMilliseconds)))

                $Result | Add-Member -NotePropertyName DisplayOutput `
                    -NotePropertyValue $display.ToArray() -Force
            }
        }
    } finally {
        $global:TraktBuildUi = $null
    }
}

function Write-TraktResultOutput {
    param([object]$Result)

    $lines = if ($null -ne $Result.PSObject.Properties['DisplayOutput']) {
        $Result.DisplayOutput
    } else {
        $Result.Output
    }
    foreach ($line in $lines) { Write-Host $line }
}

function Invoke-TraktBuildUiTests {
    $passed = 0
    $failures = [System.Collections.Generic.List[string]]::new()
    $cases = [ordered]@{
        'UI warning does not become a failure' = {
            (Get-TraktBuildStepStatus PASS 1) -ceq 'WARN' -and
                (Get-TraktBuildStepStatus FAIL 2) -ceq 'FAIL' -and
                (Get-TraktBuildStepStatus PASS 0) -ceq 'PASS'
        }
        'UI plan follows named matrix' = {
            (Get-TraktFirmwareUiSteps all) -eq
                (1 + 10 * @(Get-TraktVariantNames).Count) -and
                (Get-TraktFirmwareUiSteps full) -eq 11
        }
        'UI activity cannot alter completed stage count' = {
            for ($done = 0; $done -le 9; $done++) {
                for ($tick = 0; $tick -le 4200; $tick += 140) {
                    $bar = Get-TraktBuildBar $done 9 $tick
                    if ($bar.Length -ne 18 -or
                        [regex]::Matches($bar, '#').Count -ne 2 * $done -or
                        $bar -match '[^#=-]') {
                        throw 'Invalid progress geometry.'
                    }
                }
            }
            (Get-TraktBuildBar 9 9 0) -ceq ('#' * 18) -and
                (Get-TraktBuildBar 0 9 0) -ceq (Get-TraktBuildBar 0 9 4200)
        }
        'UI live text fits its assigned columns' = {
            $text = "row`n" + ([string][char]0x0410) + ('x' * 160)
            foreach ($width in @(0, 1, 2, 3, 4, 20, 80, 144)) {
                $line = Limit-TraktBuildLiveLine $text $width
                if ($line.Length -gt $width -or $line -match '[^\x20-\x7E]') {
                    throw 'Transient line exceeded its cell budget.'
                }
            }
            (Limit-TraktBuildLiveLine 'abcdef' 5) -ceq 'ab...' -and
                (Limit-TraktBuildLiveLine 'abc' 5) -ceq 'abc'
        }
        'UI ignores malformed JSON without authorizing it' = {
            $null -eq (ConvertFrom-TraktCargoEvent '{bad' stdout)
        }
        'UI distinguishes cached artifacts' = {
            $event = ConvertFrom-TraktCargoEvent `
                '{"reason":"compiler-artifact","target":{"name":"trakt","kind":["bin"]},"fresh":true}' stdout
            $event.Kind -ceq 'Artifact' -and $event.Fresh -and
                $event.Text -ceq 'trakt [bin] cached'
        }
        'UI observes diagnostic location and severity' = {
            $event = ConvertFrom-TraktCargoEvent `
                '{"reason":"compiler-message","message":{"level":"warning","message":"probe","spans":[{"is_primary":true,"file_name":"src/main.rs","line_start":7,"column_start":2}]}}' stdout
            $event.Warnings -eq 1 -and $event.Errors -eq 0 -and
                $event.Text -ceq 'warning: probe (src/main.rs:7:2)'
        }
        'UI observes Cargo stderr without inventing compiler phases' = {
            $event = ConvertFrom-TraktCargoEvent '   Compiling trakt v0.1.0' stderr
            $event.Text -ceq 'Compiling trakt v0.1.0' -and
                $event.Warnings -eq 0 -and $event.Errors -eq 0
        }
        'UI sanitizes transient status controls' = {
            (ConvertTo-TraktBuildLine "one`ntwo`rthree") -ceq 'one two three'
        }
    }

    foreach ($name in $cases.Keys) {
        try {
            if (& $cases[$name]) { $passed++ }
            else { $failures.Add("FAIL: $name") }
        } catch {
            $failures.Add("FAIL: $name; $($_.Exception.Message)")
        }
    }

    return (New-TraktResult -ExitCode ([int]($failures.Count -gt 0)) `
        -Passed $passed -Failed $failures.Count -Output $failures.ToArray())
}