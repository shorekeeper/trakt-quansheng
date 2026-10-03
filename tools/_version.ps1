#Requires -Version 7.3
# Reserve one identity for the selected variants and optimization profiles.

function Read-TraktVersion {
    $path = Join-Path $global:TraktRoot 'version.json'
    $state = [System.IO.File]::ReadAllText($path) | ConvertFrom-Json
    if ($state.schema -ne 1 -or $state.major -ne 0 -or
        $state.minor -lt 1 -or $state.minor -gt 9999 -or
        $state.patch -lt 0 -or $state.patch -gt 999999999 -or
        [long]$state.minor -ne $state.minor -or
        [long]$state.patch -ne $state.patch) {
        throw 'Invalid firmware version state.'
    }
    Assert-TraktVersionName -Name ([string]$state.name) -AllowEmpty
    return $state
}

function Assert-TraktVersionName {
    param([string]$Name, [switch]$AllowEmpty)

    if (-not $Name -and $AllowEmpty) { return }
    if ($Name.Length -gt 16 -or
        $Name -notmatch '^[A-Za-z0-9][A-Za-z0-9 ._-]*$' -or
        $Name -cne $Name.Trim()) {
        throw 'Version names require 1..16 ASCII letters, digits, spaces, dots, underscores, or hyphens.'
    }
}

function Get-TraktVersionText {
    param([object]$State)

    $version = "$($State.major).$($State.minor).$($State.patch)"
    $label = if ($State.name) {
        "Build $($State.name).$($State.patch)"
    } else { "Ver $version" }

    return [pscustomobject]@{ Version = $version; Label = $label }
}

function Step-TraktVersion {
    param([object]$State, [string]$Assign = '', [switch]$IgnoreIncrement)

    $next = [pscustomobject]@{
        schema = 1
        major = 0
        minor = [long]$State.minor
        patch = [long]$State.patch
        name = [string]$State.name
    }

    if ($Assign) {
        Assert-TraktVersionName $Assign
        if ($next.minor -eq 9999) { throw 'Minor version limit reached.' }
        $next.minor++
        $next.patch = 0
        $next.name = $Assign
    } elseif (-not $IgnoreIncrement) {
        if ($next.patch -eq 999999999) { throw 'Patch version limit reached.' }
        $next.patch++
    }
    return $next
}

function Enter-TraktBuildLock {
    $directory = Join-Path $global:TraktRoot 'target'
    $null = New-Item -ItemType Directory -Path $directory -Force
    try {
        return [System.IO.File]::Open(
            (Join-Path $directory 'version.lock'),
            [System.IO.FileMode]::OpenOrCreate,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::None
        )
    } catch {
        throw 'Another Trakt build or version assignment is active.'
    }
}

function Write-TraktAtomicText {
    param([string]$Path, [string]$Text)

    $temporary = "$Path.$PID.tmp"
    try {
        [System.IO.File]::WriteAllText(
            $temporary, $Text, [System.Text.UTF8Encoding]::new($false)
        )
        [System.IO.File]::Move($temporary, $Path, $true)
    } finally {
        if (Test-Path -LiteralPath $temporary) {
            Remove-Item -LiteralPath $temporary -Force
        }
    }
}

function Prepare-TraktVersion {
    param([string]$Assign = '', [switch]$IgnoreIncrement)

    $state = Step-TraktVersion (Read-TraktVersion) -Assign $Assign -IgnoreIncrement:$IgnoreIncrement
    $text = Get-TraktVersionText $state
    $json = ConvertTo-Json -InputObject $state
    Write-TraktAtomicText -Path (Join-Path $global:TraktRoot 'version.json') -Text ($json + "`n")

    $source = @"
//! Build identity displayed by boot presentation and About.
//!
//! The build command reserves this identity before compilation. Failed
//! builds retain their reserved number. Explicit ignore-increment selects
//! the current identity without allocating another number.

pub(crate) const VERSION: &[u8] = b"$($text.Version)";
pub(crate) const LABEL: &[u8] = b"$($text.Label)";
"@
    $path = Join-Path $global:TraktRoot 'src/build_info.rs'
    if (-not (Test-Path -LiteralPath $path) -or
        [System.IO.File]::ReadAllText($path) -cne ($source + "`n")) {
        Write-TraktAtomicText -Path $path -Text ($source + "`n")
    }
    return $text
}

function Read-TraktBuildOptions {
    param(
        [string[]]$Arguments,
        [string]$DefaultProfile = 'dev',
        [ValidateSet('full', 'radio', 'compact', 'standalone', 'debug', 'full-fam', 'all')][string]$DefaultVariant = 'full'
    )

    $remaining = [System.Collections.Generic.List[string]]::new()
    $assign = ''
    $ignore = $false
    $variant = $DefaultVariant
    $variantSeen = $false

    for ($index = 0; $index -lt $Arguments.Count; $index++) {
        $value = $Arguments[$index]
        if ($value -eq '--ignore-increment') {
            $ignore = $true
        } elseif ($value -eq '--assign') {
            $index++
            if ($index -ge $Arguments.Count) { throw '--assign requires a name.' }
            if ($assign) { throw 'Only one --assign is allowed.' }
            $assign = $Arguments[$index]
            Assert-TraktVersionName $assign
        } elseif ($value.StartsWith('--assign=')) {
            if ($assign) { throw 'Only one --assign is allowed.' }
            $assign = $value.Substring(9)
            Assert-TraktVersionName $assign
        } elseif ($value -eq '--variant' -or $value.StartsWith('--variant=')) {
            if ($variantSeen) { throw 'Only one --variant is allowed.' }
            $variantSeen = $true

            if ($value -eq '--variant') {
                $index++
                if ($index -ge $Arguments.Count) {
                    throw '--variant requires full, radio, compact, standalone, debug, full-fam, or all.'
                }
                $variant = $Arguments[$index]
            } else {
                $variant = $value.Substring(10)
            }

            if ($variant -cne 'all' -and $variant -cnotin @(Get-TraktVariantNames)) {
                throw 'Variant must be full, radio, compact, standalone, debug, full-fam, or all.'
            }
        } elseif ($value.StartsWith('--')) {
            throw "Unknown build option: $value"
        } else {
            $remaining.Add($value)
        }
    }

    return [pscustomobject]@{
        Assign = $assign
        IgnoreIncrement = $ignore
        Remaining = $remaining.ToArray()
        DefaultProfile = $DefaultProfile
        Variant = $variant
    }
}

function Invoke-TraktBuildCommand {
    param([string[]]$Arguments, [switch]$CheckOnly)

    $lock = $null
    try {
        $options = Read-TraktBuildOptions $Arguments
        if ($options.Remaining.Count -gt 1 -or
            ($options.Remaining.Count -eq 1 -and $options.Remaining[0] -notin 'dev', 'release')) {
            throw 'Usage: build [dev|release] [--assign=NAME] [--ignore-increment]'
        }
        if ($options.Variant -ceq 'all') {
            throw 'Use ci --variant=all for the variant matrix.'
        }
        if ($CheckOnly -and $options.Assign) {
            throw 'check cannot assign a version.'
        }

        $profile = if ($options.Remaining.Count) { $options.Remaining[0] } else { 'dev' }
        $audit = Invoke-TraktAudit
        if ($audit.ExitCode -ne 0) { return $audit }

        $lock = Enter-TraktBuildLock
        $identity = Prepare-TraktVersion -Assign $options.Assign `
            -IgnoreIncrement:($CheckOnly -or $options.IgnoreIncrement)
        Write-Host "$($identity.Version) | $($identity.Label)" -ForegroundColor DarkGray
        return (Invoke-TraktBuild -Profile $profile -CheckOnly:$CheckOnly `
            -Variant $options.Variant)
    } catch {
        return (New-TraktResult -ExitCode 1 -Output @($_.Exception.Message))
    } finally {
        if ($null -ne $lock) { $lock.Dispose() }
    }
}

function Invoke-TraktFirmwareGate {
    param(
        [string]$Assign = '',
        [switch]$IgnoreIncrement,
        [ValidateSet('full', 'radio', 'compact', 'standalone', 'debug', 'full-fam', 'all')][string]$Variant = 'all'
    )

    $owner = Start-TraktBuildUi -Label 'firmware' `
        -Steps (Get-TraktFirmwareUiSteps $Variant)
    $lock = $null
    $result = $null

    try {
        Enter-TraktBuildStep 'Firmware identity' 'Acquiring build lock'
        $lock = Enter-TraktBuildLock
        $identity = Prepare-TraktVersion -Assign $Assign -IgnoreIncrement:$IgnoreIncrement
        Exit-TraktBuildStep -Detail "$($identity.Version) | $($identity.Label)"

        $result = & {
            $variants = if ($Variant -ceq 'all') {
                @(Get-TraktVariantNames)
            } else {
                @($Variant)
            }
            $output = [System.Collections.Generic.List[string]]::new()
            $passed = 0

            foreach ($selected in $variants) {
                foreach ($profile in @('dev', 'release')) {
                    $build = Invoke-TraktBuild -Profile $profile -Variant $selected -Brief
                    foreach ($line in $build.Output) { $output.Add($line) }
                    $passed += $build.Passed

                    if ($build.ExitCode -ne 0) {
                        return (New-TraktResult -ExitCode $build.ExitCode `
                            -Status $build.Status -Passed $passed -Failed $build.Failed `
                            -Output $output.ToArray())
                    }
                }
            }

            return (New-TraktResult -Passed $passed -Output $output.ToArray())
        }
    } catch {
        $result = New-TraktResult -ExitCode 1 -Output @($_.Exception.Message)
    } finally {
        try {
            if ($null -ne $lock) { $lock.Dispose() }
        } finally {
            Finish-TraktBuildUi -Owner $owner -Result $result
        }
    }

    return $result
}

function Invoke-TraktCiCommand {
    param([string[]]$Arguments)

    try {
        $options = Read-TraktBuildOptions $Arguments -DefaultVariant all
        return (Invoke-TraktCi -Only $options.Remaining -Assign $options.Assign `
            -IgnoreIncrement:$options.IgnoreIncrement -Variant $options.Variant)
    } catch {
        return (New-TraktResult -ExitCode 2 -Output @($_.Exception.Message))
    }
}

function Invoke-TraktAssign {
    param([string[]]$Arguments)

    if ($Arguments.Count -ne 1) {
        return (New-TraktResult -ExitCode 2 -Output @('Usage: assign NAME'))
    }
    $lock = $null
    try {
        Assert-TraktVersionName $Arguments[0]
        $lock = Enter-TraktBuildLock
        $identity = Prepare-TraktVersion -Assign $Arguments[0]
        return (New-TraktResult -Output @(
            "$($identity.Version) | $($identity.Label)",
            'No compilation was performed.'
        ))
    } catch {
        return (New-TraktResult -ExitCode 1 -Output @($_.Exception.Message))
    } finally {
        if ($null -ne $lock) { $lock.Dispose() }
    }
}

function Invoke-TraktVersionTests {
    $passed = 0
    $failures = [System.Collections.Generic.List[string]]::new()
    $cases = [ordered]@{
        'Patch increment' = {
            $state = [pscustomobject]@{ minor = 1; patch = 4; name = '' }
            $next = Step-TraktVersion $state
            $next.minor -eq 1 -and $next.patch -eq 5 -and
                (Get-TraktVersionText $next).Label -ceq 'Ver 0.1.5'
        }
        'Assignment resets patch' = {
            $state = [pscustomobject]@{ minor = 1; patch = 4; name = '' }
            $next = Step-TraktVersion $state -Assign Pin
            $next.minor -eq 2 -and $next.patch -eq 0 -and
                (Get-TraktVersionText $next).Label -ceq 'Build Pin.0'
        }
        'Named build continues patch' = {
            $state = [pscustomobject]@{ minor = 2; patch = 0; name = 'Pin' }
            $next = Step-TraktVersion $state
            (Get-TraktVersionText $next).Label -ceq 'Build Pin.1'
        }
        'Explicit no increment' = {
            $state = [pscustomobject]@{ minor = 2; patch = 7; name = 'Pin' }
            (Step-TraktVersion $state -IgnoreIncrement).patch -eq 7
        }
        'Name validation' = {
            try { Assert-TraktVersionName 'Bad"Name'; $false } catch { $true }
        }
        'CI assignment parsing' = {
            $options = Read-TraktBuildOptions @('--assign=Pin', '--ignore-increment', 'firmware')
            $options.Assign -ceq 'Pin' -and $options.IgnoreIncrement -and
                $options.Remaining.Count -eq 1 -and $options.Remaining[0] -ceq 'firmware'
        }
    }

    foreach ($name in $cases.Keys) {
        try {
            if (& $cases[$name]) { $passed++ }
            else { $failures.Add("FAIL: $name") }
        } catch { $failures.Add("FAIL: $name`: $($_.Exception.Message)") }
    }
    return (New-TraktResult -ExitCode ([int]($failures.Count -gt 0)) `
        -Passed $passed -Failed $failures.Count -Output $failures.ToArray())
}