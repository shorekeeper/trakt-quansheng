#Requires -Version 7.3
# Emit diagnostic assembly in a separate target directory.

param(
    [Parameter(ValueFromRemainingArguments)]
    [AllowEmptyCollection()]
    [AllowEmptyString()]
    [string[]]$RawArgs = @()
)

$ErrorActionPreference = 'Stop'
if (-not (Get-Command Invoke-TraktAction -ErrorAction SilentlyContinue)) {
    . (Join-Path $PSScriptRoot '../tools/bootstrap.ps1')
}

try {
    $options = Read-TraktBuildOptions $RawArgs
    if ($options.Assign -or $options.IgnoreIncrement -or
        $options.Variant -ceq 'all' -or
        $options.Remaining.Count -gt 1 -or
        ($options.Remaining.Count -eq 1 -and
            $options.Remaining[0] -notin 'dev', 'release')) {
        return (New-TraktResult -ExitCode 2 -Output @(
            'Usage: frames [dev|release] [--variant=full|radio|compact|standalone|debug|full-fam]'
        ))
    }

    $profile = if ($options.Remaining.Count) {
        $options.Remaining[0]
    } else { 'release' }
    $variantInfo = Get-TraktVariant $options.Variant

    $audit = Invoke-TraktAudit
    if ($audit.ExitCode -ne 0) { return $audit }
    Assert-TraktBuildEnvironment

    $target = Join-Path $global:TraktRoot "target/frame-probe/$($variantInfo.Name)"
    $directory = Join-Path $target "assembly/$profile"
    $assembly = Join-Path $directory 'trakt.s'
    $null = New-Item -ItemType Directory -Path $directory -Force
    if (Test-Path -LiteralPath $assembly) {
        Remove-Item -LiteralPath $assembly -Force
    }

    $arguments = @(
        'rustc', '--offline', '--locked',
        '--manifest-path', (Join-Path $global:TraktRoot 'Cargo.toml'),
        '--target', 'thumbv6m-none-eabi',
        '--target-dir', $target,
        '--bin', 'trakt',
        '--profile', $profile,
        '--message-format=json'
    ) + $variantInfo.CargoArguments + @(
        '--', "--emit=link,asm=$assembly"
    )

    Write-Host "Compiler prologues: $profile / $($variantInfo.Name)" -ForegroundColor DarkGray
    $run = ConvertFrom-TraktCargo (Invoke-TraktCargo -Arguments $arguments)
    if ($run.ExitCode -ne 0) {
        return (New-TraktResult -ExitCode $run.ExitCode -Output $run.Output)
    }
    if (-not (Test-Path -LiteralPath $assembly -PathType Leaf)) {
        throw 'The compiler did not produce the requested assembly.'
    }

    $lines = [System.IO.File]::ReadAllLines($assembly)
    $output = [System.Collections.Generic.List[string]]::new()
    $found = 0

    for ($index = 0; $index -lt $lines.Length; $index++) {
        $line = $lines[$index]
        $match = [regex]::Match(
            $line,
            '^([A-Za-z_.$][A-Za-z0-9_.$]*):\s*(?:[@#].*)?$'
        )
        if (-not $match.Success) { continue }

        $name = $match.Groups[1].Value
        $wanted = $name -in @(
            '__trakt_main', '__trakt_exception', 'SysTick_Handler'
        ) -or $name -match '^_ZN5trakt3app3run' -or
            $name -match '^_ZN5trakt3app5tasks' -or
            $name -match '^_ZN5trakt3app5panel5Panel6render' -or
            $name -match '^_ZN5trakt7drivers6bk4829.*4pump' -or
            $name -match '^_ZN5trakt5debug7Monitor4poll'

        if (-not $wanted) { continue }

        $found++
        $output.Add('')
        $output.Add($name + ':')

        # Show the prologue through its first call, with a fixed output bound.
        # Register-based SP adjustments remain visible beside their literals.
        $limit = [math]::Min($lines.Length, $index + 65)
        for ($at = $index + 1; $at -lt $limit; $at++) {
            $text = $lines[$at].Trim()
            if ($text) { $output.Add('  ' + $text) }
            if ($text -match '^(bl|blx)\s' -or
                $text -match '^\.Lfunc_end\d+:') {
                break
            }
        }
    }

    if ($found -eq 0) {
        throw "No requested function labels were found. Assembly: $assembly"
    }

    $output.Add('')
    $output.Add("Assembly: $assembly")
    $output.Add('Prologues show local allocations, not maximum call-chain stack usage.')
    $output.Add('This command does not export or approve a firmware image.')

    return (New-TraktResult -Output $output.ToArray())
} catch {
    return (New-TraktResult -ExitCode 1 -Output @($_.Exception.Message))
}