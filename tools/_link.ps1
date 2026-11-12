#Requires -Version 7.3
# Execute Link protocol and bounded application-state checks on Windows.

function Invoke-TraktLinkTests {
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
            throw 'Link host checks require an installed Windows Rust target.'
        }

        $directory = Join-Path $global:TraktRoot 'target/link-host'
        $executable = Join-Path $directory 'link-tests.exe'
        $null = New-Item -ItemType Directory -Path $directory -Force
        if (Test-Path -LiteralPath $executable) {
            Remove-Item -LiteralPath $executable -Force
        }

        Set-TraktBuildDetail 'Compiling Link protocol for Windows'
        $compiled = Invoke-TraktNative -Name rustc -Arguments @(
            '--edition=2021',
            '--target', $hostMatch.Groups[1].Value,
            '--test', '-D', 'warnings',
            '-C', 'overflow-checks=yes',
            (Join-Path $global:TraktRoot 'tests/link_probe.rs'),
            '-o', $executable
        )
        if ($compiled.ExitCode -ne 0) {
            throw ($compiled.Output -join "`n")
        }
        if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
            throw 'rustc produced no Link test executable.'
        }

        Set-TraktBuildDetail 'Executing Link packet and delivery checks'
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
            throw 'The Link test runner did not report successful completion.'
        }

        return (New-TraktResult -Passed ([int]$summary.Groups[1].Value) -Output @(
            "Link host checks: $($summary.Groups[1].Value) passed.",
            'Packet and application state only; hardware transmission was not tested.'
        ))
    } catch {
        return (New-TraktResult -ExitCode 1 -Failed 1 -Output @(
            "FAIL: Link host checks: $($_.Exception.Message)"
        ))
    }
}