#Requires -Version 7.3
# Command implementations over the build, validation, inspection, and configuration tools.

function Invoke-TraktAudit {
    $errorsFound = [System.Collections.Generic.List[string]]::new()
    $files = @(Get-TraktSourceFiles)
    foreach ($file in $files) {
        Set-TraktBuildDetail ("ASCII audit: " +
            [System.IO.Path]::GetRelativePath($global:TraktRoot, $file.FullName))
        $bytes = [System.IO.File]::ReadAllBytes($file.FullName)
        if (@($bytes | Where-Object { $_ -gt 127 }).Count -gt 0) {
            $errorsFound.Add("Non-ASCII source: $($file.FullName)")
        }
    }

    $manifest = [System.IO.File]::ReadAllText((Join-Path $global:TraktRoot 'Cargo.toml'))
    $tables = [regex]::Matches($manifest, '(?m)^\s*\[[^\r\n]*dependencies[^\r\n]*\]\s*$')
    if ($tables.Count -ne 1 -or $tables[0].Value.Trim() -ne '[dependencies]') {
        $errorsFound.Add('The manifest must contain exactly one plain dependencies table.')
    }
    $dependencyBody = [regex]::Match(
        $manifest,
        '(?ms)^\[dependencies\][ \t]*\r?\n(.*?)(?=^\[|\z)'
    )
    if (-not $dependencyBody.Success) {
        $errorsFound.Add('The empty dependencies table is missing.')
    } else {
        foreach ($line in $dependencyBody.Groups[1].Value -split "`n") {
            if ($line.Trim() -and -not $line.Trim().StartsWith('#')) {
                $errorsFound.Add('External Cargo dependencies are prohibited.')
                break
            }
        }
    }
    if (Test-Path -LiteralPath (Join-Path $global:TraktRoot 'build.rs')) {
        $errorsFound.Add('build.rs is prohibited by the current build contract.')
    }
    if ($manifest -notmatch '(?m)^build\s*=\s*false\s*$') {
        $errorsFound.Add('Automatic build-script discovery must be disabled.')
    }
    if ($errorsFound.Count -gt 0) {
        return (New-TraktResult -ExitCode 1 -Failed $errorsFound.Count -Output $errorsFound.ToArray())
    }
    return (New-TraktResult -Passed 1 -Output @(
        "ASCII audit passed: $($files.Count) active files.",
        'Dependency policy passed. Input seeds and generated state were excluded.'
    ))
}

function Invoke-TraktDoctor {
    $lines = [System.Collections.Generic.List[string]]::new()
    $failures = 0
    $lines.Add("PowerShell: $($PSVersionTable.PSVersion)")
    $lines.Add(".NET: $([System.Environment]::Version)")
    $lines.Add("Windows: $IsWindows")
    if (-not $IsWindows) { $failures++; $lines.Add('Windows is required.') }

    foreach ($name in @('cargo', 'rustc')) {
        $result = Invoke-TraktNative -Name $name -Arguments @('--version')
        foreach ($line in $result.Output) { $lines.Add($line) }
        if ($result.ExitCode -ne 0) { $failures++ }
    }

    $target = Invoke-TraktNative -Name rustc -Arguments @(
        '--print', 'target-libdir', '--target', 'thumbv6m-none-eabi'
    )
    if ($target.ExitCode -eq 0 -and $target.Output.Count -eq 1) {
        $directory = $target.Output[0].Trim()
        $coreFiles = if (Test-Path -LiteralPath $directory -PathType Container) {
            @(Get-ChildItem -LiteralPath $directory -Filter 'libcore-*.rlib' -File)
        } else { @() }
        if ($coreFiles.Count -gt 0) {
            $lines.Add('Target core library: present.')
        } else {
            $failures++
            $lines.Add('Target core library: missing; install it through the selected Rust toolchain.')
        }
    } else {
        $failures++
        $lines.Add('Target library location could not be queried.')
    }

    $git = Get-Command git -CommandType Application -ErrorAction SilentlyContinue
    $lines.Add("Optional Git: $(if ($git) { 'present' } else { 'absent' })")
    $lines.Add('No installation, package download, or device access was performed.')
    return (New-TraktResult -ExitCode ([int]($failures -gt 0)) -Failed $failures -Output $lines.ToArray())
}

function Invoke-TraktStatus {
    $stats = Get-RsFileStats
    $lines = [System.Collections.Generic.List[string]]::new()
    $identity = Get-TraktVersionText (Read-TraktVersion)
    $lines.Add("Trakt $($identity.Version) | $($identity.Label)")
    $lines.Add('Target: UV-K5 V3 / PY32F071.')
    $lines.Add('Firmware: FM/NFM TX/RX, AM/NAM RX, VFO/MR, split, CTCSS/DCS, and Dual Watch.')
    $lines.Add('PTT uses the selected profile with policy, battery, carrier, and timeout checks.')
    $lines.Add('Save/Load retain settings, HAM, and DW. Reset RAM leaves saved records untouched.')
    $lines.Add('Hardware checks are performed separately; this command does not inspect the radio.')
    $lines.Add("Rust source: $($stats.Files) files, $($stats.Lines) lines, $($stats.Bytes) bytes.")
    $lines.Add('Use inspect dev or inspect release for verified artifact accounting.')
    $lines.Add('Stack: linked reservation only; device watermark is reported separately.')
    $git = Invoke-TraktNative -Name git -Arguments @('-C', $global:TraktRoot, 'status', '--short', '--branch')
    if ($git.ExitCode -eq 0) {
        foreach ($line in $git.Output) { $lines.Add($line) }
    } else { $lines.Add('Git status: unavailable or outside a repository.') }
    return (New-TraktResult -Output $lines.ToArray())
}

function Invoke-TraktToolsTest {
    $passed = 0
    $failures = [System.Collections.Generic.List[string]]::new()
    $cases = [ordered]@{
        'Quoted chain operators' = {
            $items = @(Split-TraktCommandChain 'alias x "audit && status"; help')
            $items.Count -eq 2 -and $items[1].Op -eq ';'
        }
        'Single-quoted token and empty argument' = {
            $items = @(Split-TraktTokens "alias x 'a b' ''")
            $items.Count -eq 4 -and $items[2] -eq 'a b' -and $items[3] -eq ''
        }
        'Whitespace token boundaries' = {
            $items = @(Split-TraktTokens "status`tvalue")
            $items.Count -eq 2 -and $items[1] -eq 'value'
        }
        'Doubled single quote' = {
            $items = @(Split-TraktTokens "'a''b'")
            $items.Count -eq 1 -and $items[0] -eq "a'b"
        }
        'Token round trip' = {
            $value = "a b'c;d"
            $items = @(Split-TraktTokens (ConvertTo-TraktToken $value))
            $items.Count -eq 1 -and $items[0] -eq $value
        }
        'Incomplete chain rejection' = {
            try { $null = Split-TraktCommandChain 'audit &&'; $false } catch { $true }
        }
        'Unclosed quote rejection' = {
            try { $null = Split-TraktTokens 'help "bad'; $false } catch { $true }
        }
        'Explicit result contract' = {
            $result = New-TraktResult -ExitCode 2 -Status BLOCKED
            $result.ExitCode -eq 2 -and $result.Status -eq 'BLOCKED'
        }
        'Adapter default argument array' = {
            $path = Join-Path $global:TraktRoot 'tests/argument_probe.ps1'
            $result = & $path
            $result.Count -eq 0 -and $result.Values.Count -eq 0
        }
        'Adapter explicit empty collection' = {
            $path = Join-Path $global:TraktRoot 'tests/argument_probe.ps1'
            [string[]]$values = @()
            $result = & $path -RawArgs $values
            $result.Count -eq 0 -and $result.Values.Count -eq 0
        }
        'Adapter preserves an empty string' = {
            $path = Join-Path $global:TraktRoot 'tests/argument_probe.ps1'
            [string[]]$values = @('')
            $result = & $path -RawArgs $values
            $result.Count -eq 1 -and $result.Values[0] -ceq ''
        }
        'Adapter preserves token boundaries' = {
            $path = Join-Path $global:TraktRoot 'tests/argument_probe.ps1'
            [string[]]$values = @('a b', 'c;d', 'e&&f')
            $result = & $path -RawArgs $values
            $result.Count -eq 3 -and
                $result.Values[0] -ceq 'a b' -and
                $result.Values[1] -ceq 'c;d' -and
                $result.Values[2] -ceq 'e&&f'
        }
        'Theme type survives initialization' = {
            Initialize-TraktMuxModules
            $themeType = [type]'TraktTheme'
            $themes = Get-MuxThemeTable
            $saved = @{}
            foreach ($key in $themes['dark'].Keys) {
                $property = $themeType.GetProperty($key)
                if ($null -eq $property) { throw "Missing theme property: $key" }
                $saved[$key] = $property.GetValue($null)
            }
            try {
                Set-TraktMuxTheme -Name light
                $themeType.GetProperty('Bg').GetValue($null) -eq $themes['light']['Bg']
            } finally {
                foreach ($key in $saved.Keys) {
                    $themeType.GetProperty($key).SetValue($null, [int]$saved[$key])
                }
            }
        }
        'Mux types construct outside initialization' = {
            Initialize-TraktMuxModules
            $engineType = [type]'TraktMuxEngine'
            $paneType = [type]'TraktMenuPane'
            $engine = [Activator]::CreateInstance($engineType)
            $pane = [Activator]::CreateInstance($paneType)
            $node = @{ Kind = 'pane'; Id = 'probe'; Pane = $pane }
            $engine.Root = $node
            $engine.FocusId = 'probe'
            $engine.AttachPanes()
            [object]::ReferenceEquals($pane.Engine, $engine) -and
                [object]::ReferenceEquals($engine.FocusedNode(), $node)
        }
    }

    foreach ($name in $cases.Keys) {
        try {
            if (& $cases[$name]) { $passed++ }
            else { $failures.Add("FAIL: $name") }
        } catch { $failures.Add("FAIL: $name; $($_.Exception.Message)") }
    }

    foreach ($file in Get-TraktSourceFiles | Where-Object { $_.Extension -eq '.ps1' }) {
        $tokens = $null
        $parseErrors = $null
        $null = [System.Management.Automation.Language.Parser]::ParseFile(
            $file.FullName, [ref]$tokens, [ref]$parseErrors
        )
        if ($parseErrors.Count -gt 0) {
            foreach ($errorItem in $parseErrors) {
                $failures.Add("FAIL: $($file.Name): $($errorItem.Message)")
            }
        } else { $passed++ }
    }

    foreach ($result in @(
        (Invoke-TraktElfTests),
        (Invoke-TraktStartupTests),
        (Invoke-TraktCalibrationTests),
        (Invoke-TraktVersionTests),
        (Invoke-TraktVariantTests),
        (Invoke-TraktBuildUiTests),
        (Invoke-TraktShotTests),
        (Invoke-TraktChannelTests)
    )) {
        $passed += $result.Passed
        foreach ($line in $result.Output) {
            if ($line.StartsWith('FAIL:')) { $failures.Add($line) }
        }
    }
    if ($failures.Count -gt 0) {
        return (New-TraktResult -ExitCode 1 -Passed $passed -Failed $failures.Count -Output $failures.ToArray())
    }
    return (New-TraktResult -Passed $passed -Output @(
        "Host checks passed: $passed.",
        'ELF and startup structure checks passed. Target execution was not tested.'
    ))
}

function Invoke-TraktCi {
    param(
        [string[]]$Only = @(),
        [string[]]$Skip = @(),
        [string]$Assign = '',
        [switch]$IgnoreIncrement,
        [ValidateSet('full', 'radio', 'compact', 'standalone', 'debug', 'full-fam', 'all')][string]$Variant = 'all'
    )

    $selected = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
    $excluded = @($Skip | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
    $known = @('audit', 'tools', 'firmware')
    foreach ($name in @($selected) + @($excluded)) {
        if ($name -notin $known) {
            return (New-TraktResult -ExitCode 2 -Output @("Unknown CI phase: $name"))
        }
    }

    $active = @($known | Where-Object {
        ($selected.Count -eq 0 -or $_ -in $selected) -and $_ -notin $excluded
    })
    if ($Assign -and 'firmware' -notin $active) {
        return (New-TraktResult -ExitCode 2 -Output @(
            '--assign requires the firmware phase.'
        ))
    }
    if ($active.Count -eq 0) {
        return (New-TraktResult -ExitCode 2 -Output @('No CI phase was executed.'))
    }

    $steps = @($active | Where-Object { $_ -ne 'firmware' }).Count
    if ('firmware' -in $active) {
        $steps += Get-TraktFirmwareUiSteps $Variant
    }

    $owner = Start-TraktBuildUi -Label 'CI' -Steps $steps
    $result = $null

    try {
        $rows = [System.Collections.Generic.List[string]]::new()
        $passed = 0
        $failed = 0
        $skipped = 0
        $blocked = 0

        foreach ($name in $known) {
            if ($name -notin $active) {
                $skipped++
                $rows.Add("$name : SKIP (explicit selection)")
                continue
            }

            $watch = [System.Diagnostics.Stopwatch]::StartNew()
            if ($name -ne 'firmware') {
                Enter-TraktBuildStep $name
            }

            $phase = switch ($name) {
                audit { Invoke-TraktAudit }
                tools { Invoke-TraktToolsTest }
                firmware {
                    Invoke-TraktFirmwareGate -Assign $Assign `
                        -IgnoreIncrement:$IgnoreIncrement -Variant $Variant
                }
            }
            $watch.Stop()

            if ($name -ne 'firmware') {
                $status = if ($phase.ExitCode -eq 0) { 'PASS' } else { 'FAIL' }
                $detail = if ($phase.ExitCode -eq 0) {
                    "$($phase.Passed) checks"
                } else {
                    "$($phase.Status); exit $($phase.ExitCode)"
                }
                Exit-TraktBuildStep -Status $status -Detail $detail
            }

            $rows.Add("$name : $($phase.Status) ($(Format-Duration $watch.Elapsed.TotalMilliseconds))")
            foreach ($line in $phase.Output) { $rows.Add("  $line") }

            if ($phase.Status -eq 'BLOCKED') { $blocked++ }
            elseif ($phase.ExitCode -ne 0) { $failed++ }
            else { $passed++ }
        }

        $code = if ($failed -gt 0) { 1 } elseif ($blocked -gt 0) { 2 } else { 0 }
        $status = if ($failed -gt 0) { 'FAIL' }
                  elseif ($blocked -gt 0) { 'BLOCKED' } else { 'PASS' }
        $rows.Add("CI: $status; passed=$passed failed=$failed blocked=$blocked skipped=$skipped")

        $result = New-TraktResult -ExitCode $code -Status $status `
            -Passed $passed -Failed $failed -Skipped $skipped -Output $rows.ToArray()
    } catch {
        $result = New-TraktResult -ExitCode 1 -Output @($_.Exception.Message)
    } finally {
        Finish-TraktBuildUi -Owner $owner -Result $result
    }

    return $result
}

function Invoke-TraktAction {
    param([string]$Name, [string[]]$Arguments = @())

    switch ($Name) {
        help {
            $lines = foreach ($key in $global:TraktCommands.Keys) {
                '{0,-10} {1}' -f $key, $global:TraktCommands[$key].Summary
            }
            return (New-TraktResult -Output @(
                'Trakt command shell', ''
            ) + $lines + @(
                '', 'Built-ins: clear, repeat, exit.',
                'Chains: && on success, || on failure, ; always.',
                'Available host gate: ci audit tools.',
                'Git push is a separate root script: push_git.ps1.'
            ))
        }
        assign { return (Invoke-TraktAssign -Arguments $Arguments) }
        shot { return (Invoke-TraktShot -Arguments $Arguments) }
        doctor { return (Invoke-TraktDoctor) }
        audit { return (Invoke-TraktAudit) }
        status { return (Invoke-TraktStatus) }
        { $_ -in 'build', 'check' } {
            return (Invoke-TraktBuildCommand -Arguments $Arguments -CheckOnly:($Name -eq 'check'))
        }
        inspect {
            $options = Read-TraktBuildOptions $Arguments
            if ($options.Assign -or $options.IgnoreIncrement -or
                $options.Variant -ceq 'all' -or
                $options.Remaining.Count -gt 1 -or
                ($options.Remaining.Count -eq 1 -and
                    $options.Remaining[0] -notin 'dev', 'release')) {
                return (New-TraktResult -ExitCode 2 -Output @(
                    'Usage: inspect [dev|release] [--variant=full|radio|compact|standalone|debug|full-fam]'
                ))
            }

            $profile = if ($options.Remaining.Count) {
                $options.Remaining[0]
            } else { 'release' }

            return (Invoke-TraktInspect -Profile $profile -Variant $options.Variant)
        }
        live {
            return (New-TraktResult -ExitCode 2 -Status BLOCKED -Output @(
                'Radio telemetry is not implemented. No device connection was opened.'
            ))
        }
        test {
            if ($Arguments.Count -gt 1 -or ($Arguments.Count -eq 1 -and $Arguments[0] -notin 'tools', 'all')) {
                return (New-TraktResult -ExitCode 2 -Output @('Usage: test [tools|all]'))
            }
            if ($Arguments.Count -eq 1 -and $Arguments[0] -eq 'all') { return (Invoke-TraktCi) }
            return (Invoke-TraktToolsTest)
        }
        ci { return (Invoke-TraktCiCommand -Arguments $Arguments) }
        trace {
            if ($Arguments.Count -gt 1 -or ($Arguments.Count -eq 1 -and $Arguments[0] -notin 'last', 'list')) {
                return (New-TraktResult -ExitCode 2 -Output @('Usage: trace [last|list]'))
            }
            $files = @(Get-ChildItem -LiteralPath $global:TraceDir -Filter '*.json' -File |
                Where-Object { $_.BaseName -match '^\d{8}_' } |
                Sort-Object LastWriteTimeUtc -Descending)
            if ($Arguments.Count -eq 1 -and $Arguments[0] -eq 'list') {
                return (New-TraktResult -Output @($files | ForEach-Object { $_.Name }))
            }
            if ($files.Count -eq 0) { return (New-TraktResult -Output @('No command trace exists.')) }
            $entries = @([System.IO.File]::ReadAllText($files[0].FullName) | ConvertFrom-Json)
            $entry = @($entries | Where-Object { $_.Command -ne 'trace' } | Select-Object -Last 1)
            if ($entry.Count -eq 0) { return (New-TraktResult -Output @('No non-trace command was recorded.')) }
            Format-FullTraceReport -Command $entry[0].Command -Args $entry[0].Args `
                -RawOutput $entry[0].OutputTail -DurationMs $entry[0].DurationMs -ExitCode $entry[0].ExitCode
            return (New-TraktResult -Output @("Trace file: $($files[0].Name)"))
        }
        alias {
            if ($Arguments.Count -eq 0) {
                $lines = foreach ($key in $global:UserAliases.Keys | Sort-Object) {
                    "$key = $($global:UserAliases[$key])"
                }
                return (New-TraktResult -Output @($lines))
            }
            if ($Arguments[0] -eq 'remove' -and $Arguments.Count -eq 2) {
                Remove-TraktUserAlias -Name $Arguments[1]
            } elseif ($Arguments.Count -ge 2 -and $Arguments[0] -match '^[a-zA-Z][a-zA-Z0-9_-]*$') {
                if ($Arguments[0] -in 'exit', 'clear', 'repeat') { throw 'Reserved alias name.' }
                $expansion = $Arguments[1..($Arguments.Count - 1)] -join ' '
                if ($expansion -match '[^\x20-\x7E]') { throw 'Alias expansion must be printable ASCII.' }
                $null = Split-TraktCommandChain $expansion
                Add-TraktUserAlias -Name $Arguments[0] -Expansion $expansion
            } else {
                return (New-TraktResult -ExitCode 2 -Output @(
                    'Usage: alias; alias NAME "COMMAND LINE"; alias remove NAME'
                ))
            }
            $global:UserAliases = (Import-TraktConfig).Aliases
            return (New-TraktResult -Output @('Alias configuration saved.'))
        }
        stub {
            if ($Arguments.Count -ne 0) {
                return (New-TraktResult -ExitCode 2 -Output @('Usage: stub'))
            }
            $parts = [System.Collections.Generic.List[string]]::new()
            foreach ($file in Get-ChildItem -LiteralPath (Join-Path $global:TraktRoot 'src') -Recurse -Filter '*.rs' -File |
                Sort-Object FullName) {
                $relative = [System.IO.Path]::GetRelativePath($global:TraktRoot, $file.FullName)
                $parts.Add("// FILE: $relative")
                $parts.Add((Convert-RustToStub -Path $file.FullName))
            }
            $path = Join-Path $global:TraceDir 'api.txt'
            [System.IO.File]::WriteAllText(
                $path, ($parts -join "`r`n"), [System.Text.UTF8Encoding]::new($false)
            )
            return (New-TraktResult -Output @(
                "API digest: $path",
                'Best-effort reading aid; not valid Rust and not build input.'
            ))
        }
        theme {
            if ($Arguments.Count -ne 1 -or $Arguments[0] -notin 'dark', 'light') {
                return (New-TraktResult -ExitCode 2 -Output @('Usage: theme dark|light'))
            }
            Initialize-TraktMuxModules
            Set-TraktMuxTheme -Name $Arguments[0] -Persist
            return (New-TraktResult -Output @("Theme saved: $($Arguments[0])"))
        }
        clean {
            if ($Arguments.Count -ne 2 -or $Arguments[0] -notin 'target', 'traces' -or $Arguments[1] -ne '--yes') {
                return (New-TraktResult -ExitCode 2 -Output @('Usage: clean target|traces --yes'))
            }
            $leaf = if ($Arguments[0] -eq 'target') { 'target' } else { '.trakt_trace' }
            $path = Join-Path $global:TraktRoot $leaf
            if (Test-Path -LiteralPath $path) {
                $items = @((Get-Item -LiteralPath $path)) +
                    @(Get-ChildItem -LiteralPath $path -Recurse -Force)
                if (@($items | Where-Object {
                    $_.Attributes -band [System.IO.FileAttributes]::ReparsePoint
                }).Count -gt 0) {
                    throw 'Cleanup refuses reparse points.'
                }
                Remove-Item -LiteralPath $path -Recurse -Force
            }
            if ($leaf -eq '.trakt_trace') {
                $null = New-Item -ItemType Directory -Path $global:TraceDir -Force
            }
            return (New-TraktResult -Output @("Removed generated directory: $leaf"))
        }
        default { return (New-TraktResult -ExitCode 2 -Output @("Unknown action: $Name")) }
    }
}