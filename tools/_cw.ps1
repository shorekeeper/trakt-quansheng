#Requires -Version 7.3
# Execute production CW timing and receive-window register checks on Windows.

function Invoke-TraktCwTests {
    try {
        $version = Invoke-TraktNative -Name rustc -Arguments @('-vV')
        if ($version.ExitCode -ne 0) {
            throw ($version.Output -join "`n")
        }

        $hostMatch = [regex]::Match(
            ($version.Output -join "`n"), '(?m)^host:\s*(\S+)\s*$'
        )
        if (-not $hostMatch.Success -or
            $hostMatch.Groups[1].Value -notmatch '-windows-') {
            throw 'CW host checks require an installed Windows Rust target.'
        }

        $directory = Join-Path $global:TraktRoot 'target/cw-host'
        $null = New-Item -ItemType Directory -Path $directory -Force
        $passed = 0
        $output = [System.Collections.Generic.List[string]]::new()

        foreach ($probe in @(
            @{ Source = 'cw_probe.rs'; Name = 'cw-tests'; Label = 'CW timing' },
            @{
                Source = 'cw_breakin_probe.rs'
                Name = 'cw-breakin-tests'
                Label = 'CW receive-window registers'
            }
        )) {
            $executable = Join-Path $directory ($probe.Name + '.exe')
            if (Test-Path -LiteralPath $executable) {
                Remove-Item -LiteralPath $executable -Force
            }

            Set-TraktBuildDetail ("Compiling " + $probe.Label + ' for Windows')
            $compiled = Invoke-TraktNative -Name rustc -Arguments @(
                '--edition=2021',
                '--target', $hostMatch.Groups[1].Value,
                '--test', '-D', 'warnings',
                '-C', 'overflow-checks=yes',
                (Join-Path $global:TraktRoot ('tests/' + $probe.Source)),
                '-o', $executable
            )
            if ($compiled.ExitCode -ne 0) {
                throw ($compiled.Output -join "`n")
            }
            if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
                throw 'rustc produced no CW test executable.'
            }

            Set-TraktBuildDetail ("Executing " + $probe.Label + ' on Windows')
            $executed = Invoke-TraktNative -Name $executable -Arguments @(
                '--test-threads=1'
            )
            if ($executed.ExitCode -ne 0) {
                throw ($executed.Output -join "`n")
            }

            $summary = [regex]::Match(
                ($executed.Output -join "`n"),
                'test result: ok\. (\d+) passed; 0 failed;'
            )
            if (-not $summary.Success) {
                throw 'The CW test runner did not report successful completion.'
            }
            $count = [int]$summary.Groups[1].Value
            $passed += $count
            $output.Add("$($probe.Label): $count passed.")
        }

        $output.Add('Host timing and register models only; radio behavior was not tested.')
        return (New-TraktResult -Passed $passed -Output $output.ToArray())
    } catch {
        return (New-TraktResult -ExitCode 1 -Failed 1 -Output @(
            "FAIL: CW host checks: $($_.Exception.Message)"
        ))
    }
}