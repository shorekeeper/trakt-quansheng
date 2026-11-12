#Requires -Version 7.3
# Execute production VOX software on the installed Windows Rust host target.

function Invoke-TraktVoxTests {
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
            throw 'VOX host checks require an installed Windows Rust target.'
        }

        $directory = Join-Path $global:TraktRoot 'target/vox-host'
        $executable = Join-Path $directory 'vox-tests.exe'
        $null = New-Item -ItemType Directory -Path $directory -Force
        if (Test-Path -LiteralPath $executable) {
            Remove-Item -LiteralPath $executable -Force
        }

        Set-TraktBuildDetail 'Compiling production VOX software for Windows'
        $compiled = Invoke-TraktNative -Name rustc -Arguments @(
            '--edition=2021',
            '--target', $hostMatch.Groups[1].Value,
            '--test', '-D', 'warnings',
            '-C', 'overflow-checks=yes',
            (Join-Path $global:TraktRoot 'tests/vox_probe.rs'),
            '-o', $executable
        )
        if ($compiled.ExitCode -ne 0) {
            throw ($compiled.Output -join "`n")
        }
        if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
            throw 'rustc produced no VOX test executable.'
        }

        Set-TraktBuildDetail 'Executing VOX threshold and request checks'
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
            throw 'The VOX test runner did not report successful completion.'
        }
        return (New-TraktResult -Passed ([int]$summary.Groups[1].Value) -Output @(
            "VOX host checks: $($summary.Groups[1].Value) passed.",
            'Synthetic amplitude input only; hardware VOX was not tested.'
        ))
    } catch {
        return (New-TraktResult -ExitCode 1 -Failed 1 -Output @(
            "FAIL: VOX host checks: $($_.Exception.Message)"
        ))
    }
}