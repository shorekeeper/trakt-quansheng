#Requires -Version 7.3
# Flash attribution through an analysis link with an enlarged Flash region.
# The probe output is never exported or validated as an installable image.

function Get-TraktSymbolPath {
    param([string]$Name)

    $segments = [System.Collections.Generic.List[string]]::new()
    if (-not $Name.StartsWith('_ZN')) { return ,$segments }

    $at = 3
    while ($at -lt $Name.Length -and $Name[$at] -ne 'E') {
        $start = $at
        while ($at -lt $Name.Length -and [char]::IsDigit($Name[$at])) { $at++ }
        if ($at -eq $start) { break }
        $length = [int]($Name.Substring($start, $at - $start))
        if ($at + $length -gt $Name.Length) { break }
        $segments.Add($Name.Substring($at, $length))
        $at += $length
    }

    # Legacy mangling ends with a hash segment; it carries no path meaning.
    if ($segments.Count -gt 0 -and
        $segments[$segments.Count - 1] -match '^h[0-9a-f]{16}$') {
        $segments.RemoveAt($segments.Count - 1)
    }
    return ,$segments
}

function Get-TraktSymbolModule {
    param([string]$Name)

    if (-not $Name.StartsWith('_ZN')) { return '(non-Rust)' }
    $segments = Get-TraktSymbolPath $Name

    # The final segment names the item; a leading uppercase segment is a type.
    $path = [System.Collections.Generic.List[string]]::new()
    for ($index = 0; $index -lt $segments.Count - 1; $index++) {
        if ($segments[$index] -cnotmatch '^[a-z_][a-z0-9_]*$') { break }
        $path.Add($segments[$index])
    }
    if ($path.Count -eq 0) { return '(trait impl)' }
    return $path -join '::'
}

function Format-TraktSymbolName {
    param([string]$Name)

    if (-not $Name.StartsWith('_ZN')) { return $Name }
    $text = (Get-TraktSymbolPath $Name) -join '::'
    return $text.Replace('$LT$', '<').Replace('$GT$', '>').
        Replace('$u20$', ' ').Replace('..', '::')
}

function Invoke-TraktSize {
    param([string[]]$Arguments)

    try {
        [string[]]$rest = @($Arguments | Where-Object { $_ -cne '--no-overflow-checks' })
        $unchecked = $rest.Count -ne $Arguments.Count
        $options = Read-TraktBuildOptions $rest
        if ($options.Assign -or $options.IgnoreIncrement -or
            $options.Variant -ceq 'all' -or
            $options.Remaining.Count -gt 1 -or
            ($options.Remaining.Count -eq 1 -and
                $options.Remaining[0] -notin 'dev', 'release')) {
            return (New-TraktResult -ExitCode 2 -Output @(
                'Usage: size [dev|release] [--variant=NAME] [--no-overflow-checks]'
            ))
        }

        $profile = if ($options.Remaining.Count) {
            $options.Remaining[0]
        } else { 'release' }
        $variantInfo = Get-TraktVariant $options.Variant

        $audit = Invoke-TraktAudit
        if ($audit.ExitCode -ne 0) { return $audit }
        Assert-TraktBuildEnvironment

        $layout = Get-TraktLayout
        $suffix = if ($unchecked) { '-unchecked' } else { '' }
        $directory = Join-Path $global:TraktRoot (
            "target/size-probe/$($variantInfo.Name)$suffix"
        )
        $null = New-Item -ItemType Directory -Path $directory -Force

        # The probe script differs from link/firmware.ld only in Flash length.
        # All section placement and assertions are retained.
        $region = 'FLASH (rx) : ORIGIN = 0x{0:X8}, LENGTH = {1}K' -f
            $layout.FlashOrigin, [long]($layout.FlashSize / 1024)
        $source = [System.IO.File]::ReadAllText(
            (Join-Path $global:TraktRoot 'link/firmware.ld')
        )
        if ([regex]::Matches($source, [regex]::Escape($region)).Count -ne 1) {
            throw "Flash region declaration differs from the project layout: $region"
        }
        $script = Join-Path $directory 'probe.ld'
        [System.IO.File]::WriteAllText(
            $script,
            $source.Replace($region, (
                'FLASH (rx) : ORIGIN = 0x{0:X8}, LENGTH = 1024K' -f $layout.FlashOrigin
            )),
            [System.Text.Encoding]::ASCII
        )

        # Mirrors the target rustflags of .cargo/config.toml with the probe
        # script. CARGO_ENCODED_RUSTFLAGS replaces those configured flags.
        $flags = [System.Collections.Generic.List[string]]::new()
        foreach ($item in @(
            '-C', 'target-cpu=cortex-m0plus',
            '-C', ('link-arg=-T' + $script.Replace('\', '/'))
        )) { $flags.Add($item) }
        if ($unchecked) {
            $flags.Add('-C')
            $flags.Add('overflow-checks=off')
        }

        $cargoArguments = @(
            'build', '--offline', '--locked',
            '--manifest-path', (Join-Path $global:TraktRoot 'Cargo.toml'),
            '--target', 'thumbv6m-none-eabi',
            '--target-dir', $directory,
            '--bin', 'trakt', '--profile', $profile, '--message-format=json'
        ) + $variantInfo.CargoArguments

        Write-Host "Size probe: $profile / $($variantInfo.Name)$suffix" -ForegroundColor DarkGray
        $run = ConvertFrom-TraktCargo (Invoke-TraktCargo -Arguments $cargoArguments `
            -Environment @{ CARGO_ENCODED_RUSTFLAGS = ($flags -join [char]0x1F) })
        if ($run.ExitCode -ne 0) {
            return (New-TraktResult -ExitCode $run.ExitCode -Output $run.Output)
        }
        if ($run.Artifacts.Count -ne 1) {
            throw 'Expected exactly one Trakt executable artifact.'
        }

        $limits = Get-TraktLayout
        $limits.FlashSize = [long](1024 * 1024)
        $elf = Read-TraktElf -Bytes ([System.IO.File]::ReadAllBytes($run.Artifacts[0])) `
            -Limits $limits

        # .data symbols also occupy Flash through their initializer image.
        $flash = @('.vector_table', '.text', '.rodata', '.ARM.exidx', '.data')
        $modules = @{}
        $attributed = 0L
        foreach ($symbol in $elf.Symbols) {
            if ($symbol.Section -cnotin $flash) { continue }
            $module = Get-TraktSymbolModule $symbol.Name
            if (-not $modules.ContainsKey($module)) { $modules[$module] = 0L }
            $modules[$module] += $symbol.Size
            $attributed += $symbol.Size
        }

        $used = $elf.Memory.BinBytes
        $capacity = $layout.FlashSize
        $culture = [System.Globalization.CultureInfo]::InvariantCulture
        $lines = [System.Collections.Generic.List[string]]::new()
        $scope = if ($unchecked) { ' without overflow checks' } else { '' }
        $lines.Add("Analysis link $profile/$($variantInfo.Name)$scope; not an installable image.")
        $state = if ($used -le $capacity) {
            "free $($capacity - $used) B"
        } else {
            "OVER by $($used - $capacity) B"
        }
        $lines.Add("Flash image $used B of $capacity B; $state")
        $lines.Add(("Sized symbols $attributed B; unattributed $($used - $attributed) B " +
            '(anonymous strings, panic locations, alignment)'))
        $lines.Add('')
        $lines.Add('Largest modules by sized Flash symbols:')
        foreach ($entry in @($modules.GetEnumerator() |
            Sort-Object Value -Descending | Select-Object -First 28)) {
            $share = (100.0 * $entry.Value / $used).ToString('0.0', $culture)
            $lines.Add(('  {0,7} B  {1,5}%  {2}' -f $entry.Value, $share, $entry.Key))
        }
        $lines.Add('')
        $lines.Add('Largest Flash symbols:')
        foreach ($symbol in @($elf.Symbols | Where-Object {
            $_.Section -cin $flash
        } | Select-Object -First 16)) {
            $lines.Add(('  {0,7} B  {1}' -f $symbol.Size, (Format-TraktSymbolName $symbol.Name)))
        }
        $lines.Add('')
        $lines.Add("Probe ELF: $($run.Artifacts[0])")
        $lines.Add('No version was reserved and no artifact was exported.')
        return (New-TraktResult -Output $lines.ToArray())
    } catch {
        return (New-TraktResult -ExitCode 1 -Output @($_.Exception.Message))
    }
}