#Requires -Version 7.3
# Execute the production codec on the installed Rust Windows host target.

function Invoke-TraktMessengerTests {
    try {
        $version = Invoke-TraktNative -Name rustc -Arguments @('-vV')
        if ($version.ExitCode -ne 0) {
            throw ($version.Output -join "`n")
        }

        $hostMatch = [regex]::Match(
            ($version.Output -join "`n"), '(?m)^host:\s*(\S+)\s*$'
        )
        if (-not $hostMatch.Success) {
            throw 'rustc did not report its host target.'
        }

        $target = $hostMatch.Groups[1].Value
        if ($target -notmatch '-windows-') {
            throw 'Messenger host checks require the installed Windows target.'
        }

        $directory = Join-Path $global:TraktRoot 'target/messenger-host'
        $executable = Join-Path $directory 'messenger-tests.exe'
        $null = New-Item -ItemType Directory -Path $directory -Force

        # A failed compiler invocation cannot leave an older test executable
        # eligible for execution. Generated files stay under target.
        if (Test-Path -LiteralPath $executable) {
            Remove-Item -LiteralPath $executable -Force
        }

        Set-TraktBuildDetail 'Compiling production messenger codec for Windows'
        $compiled = Invoke-TraktNative -Name rustc -Arguments @(
            '--edition=2021',
            '--target', $target,
            '--test',
            '-D', 'warnings',
            '-C', 'overflow-checks=yes',
            (Join-Path $global:TraktRoot 'tests/messenger_probe.rs'),
            '-o', $executable
        )

        if ($compiled.ExitCode -ne 0) {
            throw ($compiled.Output -join "`n")
        }
        if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
            throw 'rustc produced no messenger test executable.'
        }

        Set-TraktBuildDetail 'Executing messenger codec checks on Windows'
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
            throw 'The messenger test runner did not report checked completion.'
        }

        $passed = [int]$summary.Groups[1].Value
        return (New-TraktResult -Passed $passed -Output @(
            "Messenger host checks: $passed passed.",
            'Production codec executed on Windows; no audio or RF test was performed.'
        ))
    } catch {
        return (New-TraktResult -ExitCode 1 -Failed 1 -Output @(
            "FAIL: messenger host checks: $($_.Exception.Message)"
        ))
    }
}