#Requires -Version 7.3
# Named firmware compositions, independent of Cargo optimization profiles.

function Get-TraktOptionalDiagnosticRecords {
    # debug-tools owns these optional fixed debugger snapshots.
    # Basic diagnostic screens use operational or mandatory records instead.
    # Stack/fault/TX-limit records and operational buffers are not optional.
    return @(
        'TRAKT_BOOT',
        'TRAKT_RUNTIME',
        'TRAKT_BENCH',
        'TRAKT_DISPLAY',
        'TRAKT_INPUT',
        'TRAKT_ADC',
        'TRAKT_STORAGE',
        'TRAKT_RECEIVER',
        'TRAKT_SQUELCH',
        'TRAKT_CONTROL',
        'TRAKT_JOURNAL',
        'TRAKT_TRANSMITTER',
        'TRAKT_TIMING'
    )
}

function Get-TraktVariantTable {
    return @(
        [pscustomobject]@{
            Name = 'full'
            Features = [string[]]@('diagnostics', 'spectrum', 'uart')
        },
        [pscustomobject]@{
            Name = 'radio'
            Features = [string[]]@('spectrum', 'uart')
        },
        [pscustomobject]@{
            Name = 'compact'
            Features = [string[]]@('uart')
        },
        [pscustomobject]@{
            Name = 'standalone'
            Features = [string[]]@()
        },
        [pscustomobject]@{
            Name = 'debug'
            Features = [string[]]@('diagnostics', 'debug-tools', 'uart')
        },
        [pscustomobject]@{
            Name = 'full-fam'
            Features = [string[]]@('diagnostics', 'spectrum', 'uart', 'fam')
        }
    )
}

function Get-TraktUartRecords {
    # Current UART composition owns the DMA ring, staged input, stable
    # binary reply buffer, and serial diagnostic publication.
    return @(
        'TRAKT_UART_RX',
        'TRAKT_CHANNEL_RX',
        'TRAKT_CAPTURE',
        'TRAKT_SERIAL'
    )
}

function Get-TraktVariantNames {
    foreach ($row in Get-TraktVariantTable) {
        $row.Name
    }
}

function Get-TraktVariant {
    param([string]$Name = 'full')

    $rows = @(Get-TraktVariantTable | Where-Object { $_.Name -ceq $Name })
    if ($rows.Count -ne 1) {
        throw "Unknown firmware variant: $Name"
    }

    [string[]]$features = @($rows[0].Features)
    [string[]]$arguments = @('--no-default-features')
    if ($features.Count -gt 0) {
        $arguments += @('--features', ($features -join ','))
    }

    return [pscustomobject]@{
        Name = $Name
        Features = $features
        CargoArguments = $arguments
    }
}

function Invoke-TraktVariantTests {
    $passed = 0
    $failures = [System.Collections.Generic.List[string]]::new()

    $cases = [ordered]@{
        'Named compositions select exact explicit features' = {
            $expected = [ordered]@{
                full = 'diagnostics,spectrum,uart'
                radio = 'spectrum,uart'
                compact = 'uart'
                standalone = ''
                debug = 'diagnostics,debug-tools,uart'
                'full-fam' = 'diagnostics,spectrum,uart,fam'
            }

            $names = @(Get-TraktVariantNames)
            if (($names -join ',') -cne ($expected.Keys -join ',')) {
                throw 'Named composition matrix differs from its contract.'
            }

            foreach ($name in $expected.Keys) {
                $variant = Get-TraktVariant $name
                $features = $expected[$name]
                $arguments = if ($features) {
                    "--no-default-features --features $features"
                } else {
                    '--no-default-features'
                }
                if (($variant.Features -join ',') -cne $features -or
                    ($variant.CargoArguments -join ' ') -cne $arguments) {
                    throw "Incorrect explicit feature selection: $name"
                }
            }
            $true
        }
        'Debug has extended tools and UART but no Spectrum' = {
            $features = (Get-TraktVariant debug).Features
            $features -ccontains 'diagnostics' -and
                $features -ccontains 'debug-tools' -and
                $features -ccontains 'uart' -and
                $features -cnotcontains 'spectrum'
        }
        'Normal full retains basic diagnostics without optional snapshots' = {
            $features = (Get-TraktVariant full).Features
            $features -ccontains 'diagnostics' -and
                $features -ccontains 'spectrum' -and
                $features -cnotcontains 'debug-tools'
        }
        'Variant artifacts remain separate' = {
            $paths = @(
                foreach ($name in Get-TraktVariantNames) {
                    foreach ($profile in @('dev', 'release')) {
                        (Get-TraktArtifactPaths -Profile $profile -Variant $name).Bin
                    }
                }
            )
            $paths.Count -eq 12 -and
                @($paths | Sort-Object -Unique).Count -eq 12 -and
                (Get-TraktArtifactPaths -Profile release -Variant full).Bin -ceq
                    (Join-Path $global:TraktRoot 'target/artifacts/release/trakt.bin')
        }
        'Parser keeps variant separate from optimization profile' = {
            foreach ($name in Get-TraktVariantNames) {
                $options = Read-TraktBuildOptions @('release', "--variant=$name")
                if ($options.Variant -cne $name -or
                    $options.Remaining.Count -ne 1 -or
                    $options.Remaining[0] -cne 'release') {
                    throw "Variant parser changed the profile: $name"
                }
            }
            $true
        }
        'Default CI selects the complete matrix' = {
            $options = Read-TraktBuildOptions @() -DefaultVariant all
            $options.Variant -ceq 'all' -and
                (Get-TraktFirmwareUiSteps all) -eq 61 -and
                (Get-TraktFirmwareUiSteps debug) -eq 11
        }
        'Every named fixture passes with correct optional accounting' = {
            foreach ($name in Get-TraktVariantNames) {
                $features = (Get-TraktVariant $name).Features
                $elf = New-TraktStartupFixture -Variant $name
                Assert-TraktStartupImage -Elf $elf -Variant $name

                $diagnostic = if ($features -ccontains 'debug-tools') { 832 } else { 0 }
                $scope = if ($features -ccontains 'spectrum') { 256 } else { 0 }
                $uart = if ($features -ccontains 'uart') { 1688 } else { 0 }

                if ($elf.Memory.DiagnosticRamBytes -ne $diagnostic -or
                    $elf.Memory.SpectrumRamBytes -ne $scope -or
                    $elf.Memory.UartRamBytes -ne $uart -or
                    $elf.Memory.RetainedRamBytes -ne 64 -or
                    $elf.Memory.MainStackBudgetBytes -ne 7168 -or
                    $elf.Memory.FaultStackBudgetBytes -ne 1024) {
                    throw "Incorrect optional or mandatory accounting: $name"
                }
            }
            $true
        }
        'Selected modules require records and unselected modules reject leaks' = {
            $debug = New-TraktStartupFixture -Variant debug
            $full = New-TraktStartupFixture -Variant full
            $pool = @($debug.Symbols) + @($full.Symbols | Where-Object {
                $_.Name -ceq 'TRAKT_SCOPE'
            })
            $modules = @(
                @{
                    Feature = 'debug-tools'
                    Names = @(Get-TraktOptionalDiagnosticRecords)
                },
                @{
                    Feature = 'spectrum'
                    Names = @('TRAKT_SCOPE')
                },
                @{
                    Feature = 'uart'
                    Names = @(Get-TraktUartRecords)
                }
            )

            foreach ($name in Get-TraktVariantNames) {
                $features = (Get-TraktVariant $name).Features
                foreach ($module in $modules) {
                    foreach ($record in $module.Names) {
                        $elf = New-TraktStartupFixture -Variant $name
                        if ($features -ccontains $module.Feature) {
                            $present = @($elf.Symbols | Where-Object {
                                $_.Name -ceq $record
                            })
                            if ($present.Count -ne 1) {
                                throw "Required fixture record missing: $name/$record"
                            }
                            $elf.Symbols = @($elf.Symbols | Where-Object {
                                $_.Name -cne $record
                            })
                        } else {
                            $source = @($pool | Where-Object { $_.Name -ceq $record })
                            if ($source.Count -ne 1) {
                                throw "Leak fixture is not unique: $record"
                            }
                            $elf.Symbols = @($elf.Symbols) + @($source)
                        }

                        $rejected = $false
                        try {
                            Assert-TraktStartupImage -Elf $elf -Variant $name
                        } catch {
                            $rejected = $true
                        }
                        if (-not $rejected) {
                            throw "Incorrect optional record presence accepted: $name/$record"
                        }
                    }
                }
            }
            $true
        }
        'Every composition retains mandatory protection and operational records' = {
            $mandatory = @(
                'TRAKT_FAULT', 'TRAKT_LAST_FAULT', 'TRAKT_STACK',
                'TRAKT_TX_LIMIT', 'TRAKT_TICKS',
                'TRAKT_DATA_CHECK', 'TRAKT_BSS_CHECK',
                'TRAKT_FRAME', 'TRAKT_CALIBRATION', 'TRAKT_CHANNELS',
                'TRAKT_JOURNAL_WORK', 'TRAKT_CLEAN_SETTINGS', 'TRAKT_TX_METER'
            )
            foreach ($name in Get-TraktVariantNames) {
                foreach ($record in $mandatory) {
                    $elf = New-TraktStartupFixture -Variant $name
                    $present = @($elf.Symbols | Where-Object {
                        $_.Name -ceq $record
                    })
                    if ($present.Count -ne 1) {
                        throw "Mandatory fixture record missing: $name/$record"
                    }
                    $elf.Symbols = @($elf.Symbols | Where-Object {
                        $_.Name -cne $record
                    })

                    $rejected = $false
                    try {
                        Assert-TraktStartupImage -Elf $elf -Variant $name
                    } catch {
                        $rejected = $true
                    }
                    if (-not $rejected) {
                        throw "Missing mandatory record accepted: $name/$record"
                    }
                }
            }
            $true
        }
        'Unknown variant is rejected' = {
            try {
                $null = Read-TraktBuildOptions @('--variant=unknown')
                $false
            } catch { $true }
        }
        'Repeated variant is rejected' = {
            try {
                $null = Read-TraktBuildOptions @('--variant=full', '--variant=debug')
                $false
            } catch { $true }
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