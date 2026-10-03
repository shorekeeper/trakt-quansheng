#Requires -Version 7.3
# Structural checks for the startup diagnostic. No target code is executed.

function Assert-TraktStartupImage {
    param(
        [object]$Elf,
        [ValidateSet('full', 'radio', 'compact', 'standalone', 'debug', 'full-fam')][string]$Variant = 'full'
    )

    $optional = @(Get-TraktOptionalDiagnosticRecords)
    $features = (Get-TraktVariant $Variant).Features
    # Optional snapshot storage follows debug-tools, not basic UI navigation.
    $diagnostics = $features -ccontains 'debug-tools'
    $spectrum = $features -ccontains 'spectrum'
    $uart = $features -ccontains 'uart'
    $uartNames = @(Get-TraktUartRecords)
    $uartRecords = @($Elf.Symbols | Where-Object { $_.Name -cin $uartNames })

    if (-not $uart -and $uartRecords.Count -ne 0) {
        throw 'Disabled UART or command storage remains in image.'
    }

    $scopeRecords = @($Elf.Symbols | Where-Object { $_.Name -ceq 'TRAKT_SCOPE' })
    if (-not $spectrum -and $scopeRecords.Count -ne 0) {
        throw 'Disabled Spectrum buffer remains in image.'
    }

    if (-not $diagnostics) {
        foreach ($name in $optional) {
            $present = @($Elf.Symbols | Where-Object { $_.Name -ceq $name })
            if ($present.Count -ne 0) {
                throw "Disabled diagnostic record remains in image: $name"
            }
        }
    }

    $layout = Get-TraktLayout
    $vectors = @($Elf.Sections | Where-Object { $_.Name -ceq '.vector_table' })
    $text = @($Elf.Sections | Where-Object { $_.Name -ceq '.text' })
    $retained = @($Elf.Sections | Where-Object { $_.Name -ceq '.noinit' })

    if ($vectors.Count -ne 1 -or $vectors[0].Type -ne 1 -or
        $vectors[0].Address -ne $layout.FlashOrigin -or
        $vectors[0].Size -ne $layout.VectorBytes -or
        ($vectors[0].Flags -band 2) -eq 0) {
        throw 'Invalid startup vector section.'
    }
    if ($text.Count -ne 1 -or $text[0].Type -ne 1 -or
        ($text[0].Flags -band 6) -ne 6 -or $text[0].Size -lt 2) {
        throw 'Startup code section is missing or non-executable.'
    }
    if ($retained.Count -ne 1 -or $retained[0].Type -ne 8 -or
        $retained[0].Size -ne 64 -or ($retained[0].Flags -band 3) -ne 3) {
        throw 'Invalid retained fault record section.'
    }

    if ($retained[0].Address -ne $layout.RetainedOrigin -or
        $retained[0].Size -ne $layout.RetainedSize) {
        throw 'The fault record does not occupy the retained reservation.'
    }
    Assert-TraktSpan $retained[0].Address $retained[0].Size `
        $layout.RamOrigin $layout.RamSize 'fault record'

    $retainedOwners = @($Elf.Segments | Where-Object {
        $_.Region -ceq 'Retained'
    })
    if ($retainedOwners.Count -ne 1) {
        throw 'Expected one separate retained RAM segment.'
    }

    foreach ($segment in $Elf.Segments) {
        if ($segment.FileSize -eq 0) { continue }
        $start = $segment.Address
        $end = $start + $segment.FileSize
        if ($start -lt $retained[0].Address + $retained[0].Size -and
            $end -gt $retained[0].Address) {
            throw 'Retained memory has a file initializer.'
        }
    }

    $reserved = @(4, 5, 6, 7, 8, 9, 10, 12, 13, 46)
    for ($index = 1; $index -lt 48; $index++) {
        $value = Get-TraktU32 $Elf.Image ($index * 4)
        if ($index -in $reserved) {
            if ($value -ne 0) { throw "Reserved vector $index is nonzero." }
            continue
        }

        $address = ($value -shr 1) -shl 1
        if (($value -band 1) -eq 0 -or
            $address -lt $text[0].Address -or
            $address + 2 -gt $text[0].Address + $text[0].Size) {
            throw "Vector $index does not point to Thumb code."
        }
    }

    $expected = [ordered]@{
        TRAKT_DATA_CHECK = @{ Size = 4; Section = '.data' }
        TRAKT_BSS_CHECK = @{ Size = 4; Section = '.bss' }
        TRAKT_BOOT = @{ Size = 64; Section = '.bss' }
        TRAKT_FAULT = @{ Size = 64; Section = '.noinit' }
    }
    if (-not $diagnostics) {
        $null = $expected.Remove('TRAKT_BOOT')
    }
    foreach ($name in $expected.Keys) {
        $matches = @($Elf.Symbols | Where-Object { $_.Name -ceq $name })
        if ($matches.Count -ne 1 -or
            $matches[0].Size -ne $expected[$name].Size -or
            $matches[0].Section -cne $expected[$name].Section) {
            throw "Invalid startup diagnostic symbol: $name"
        }
    }

    $reset = @($Elf.Symbols | Where-Object { $_.Name -ceq 'Reset_Handler' })
    if ($reset.Count -ne 1 -or $reset[0].Kind -cne 'function' -or
        (($reset[0].Address -shr 1) -shl 1) -ne (($Elf.Entry -shr 1) -shl 1)) {
        throw 'Entry does not identify Reset_Handler.'
    }

    foreach ($requirement in @(
        @{ Name = 'TRAKT_TICKS'; Size = 4 },
        @{ Name = 'TRAKT_RUNTIME'; Size = 64 },
        @{ Name = 'TRAKT_FRAME'; Size = 1024 },
        @{ Name = 'TRAKT_DISPLAY'; Size = 64 },
        @{ Name = 'TRAKT_BENCH'; Size = 64 },
        @{ Name = 'TRAKT_INPUT'; Size = 64 },
        @{ Name = 'TRAKT_ADC'; Size = 64 },
        @{ Name = 'TRAKT_CALIBRATION'; Size = 512 },
        @{ Name = 'TRAKT_STORAGE'; Size = 64 },
        @{ Name = 'TRAKT_RECEIVER'; Size = 64 },
        @{ Name = 'TRAKT_SQUELCH'; Size = 64 },
        @{ Name = 'TRAKT_CONTROL'; Size = 64 },
        @{ Name = 'TRAKT_CHANNELS'; Size = 320 },
        @{ Name = 'TRAKT_JOURNAL_WORK'; Size = 1024 },
        @{ Name = 'TRAKT_JOURNAL'; Size = 64 },
        @{ Name = 'TRAKT_TRANSMITTER'; Size = 64 },
        @{ Name = 'TRAKT_TX_LIMIT'; Size = 16 },
        @{ Name = 'TRAKT_STACK'; Size = 64 },
        @{ Name = 'TRAKT_LAST_FAULT'; Size = 64 },
        @{ Name = 'TRAKT_TIMING'; Size = 64 },
        @{ Name = 'TRAKT_CAPTURE'; Size = 1044 },
        @{ Name = 'TRAKT_UART_RX'; Size = 256 },
        @{ Name = 'TRAKT_SERIAL'; Size = 64 },
        @{ Name = 'TRAKT_CLEAN_SETTINGS'; Size = 592 },
        @{ Name = 'TRAKT_SCOPE'; Size = 256 },
        @{ Name = 'TRAKT_CHANNEL_RX'; Size = 324 },
        @{ Name = 'TRAKT_TX_METER'; Size = 96 }
    )) {
        if (-not $diagnostics -and $requirement.Name -cin $optional) {
            continue
        }
        if (-not $spectrum -and $requirement.Name -ceq 'TRAKT_SCOPE') {
            continue
        }
        if (-not $uart -and $requirement.Name -cin $uartNames) {
            continue
        }
        $matches = @($Elf.Symbols | Where-Object { $_.Name -ceq $requirement.Name })
        if ($matches.Count -ne 1 -or
            $matches[0].Size -ne $requirement.Size -or
            $matches[0].Kind -cne 'object' -or
            $matches[0].Section -cne '.bss') {
            throw "Invalid runtime diagnostic symbol: $($requirement.Name)"
        }
    }

    $tickHandler = @($Elf.Symbols | Where-Object { $_.Name -ceq 'SysTick_Handler' })
    if ($tickHandler.Count -ne 1 -or
        $tickHandler[0].Kind -cne 'function' -or
        $tickHandler[0].Section -cne '.text' -or
        (($tickHandler[0].Address -shr 1) -shl 1) -ne
        (((Get-TraktU32 $Elf.Image 60) -shr 1) -shl 1)) {
        throw 'SysTick vector does not identify SysTick_Handler.'
    }

    # A subset of StaticRamBytes, not an additional allocation.
    $uartBytes = 0L
    foreach ($symbol in $uartRecords) {
        $uartBytes += $symbol.Size
    }
    $Elf.Memory | Add-Member -NotePropertyName UartRamBytes `
        -NotePropertyValue $uartBytes -Force

    $scopeBytes = 0L
    foreach ($symbol in $scopeRecords) {
        $scopeBytes += $symbol.Size
    }
    $Elf.Memory | Add-Member -NotePropertyName SpectrumRamBytes `
        -NotePropertyValue $scopeBytes -Force

    $diagnosticBytes = 0L
    foreach ($symbol in $Elf.Symbols) {
        if ($symbol.Name -cin $optional) {
            $diagnosticBytes += $symbol.Size
        }
    }
    $Elf.Memory | Add-Member -NotePropertyName DiagnosticRamBytes `
        -NotePropertyValue $diagnosticBytes -Force

    $Elf.Memory | Add-Member -NotePropertyName RetainedRamBytes `
        -NotePropertyValue $retained[0].Size -Force
    $Elf.Memory | Add-Member -NotePropertyName MainStackBudgetBytes `
        -NotePropertyValue ($layout.StackSize - $layout.FaultStackSize) -Force
    $Elf.Memory | Add-Member -NotePropertyName FaultStackBudgetBytes `
        -NotePropertyValue $layout.FaultStackSize -Force
}

function New-TraktStartupFixture {
    param(
        [ValidateSet('full', 'radio', 'compact', 'standalone', 'debug', 'full-fam')]
        [string]$Variant = 'full'
    )

    $layout = Get-TraktLayout
    [byte[]]$bytes = [byte[]]::new(640)
    $seed = New-TraktTestElf
    [System.Array]::Copy($seed, $bytes, $seed.Length)

    $entry = $layout.FlashOrigin + 193
    Set-TraktTestU32 $bytes 24 $entry
    Set-TraktTestU32 $bytes (52 + 16) 208
    Set-TraktTestU32 $bytes (52 + 20) 208
    Set-TraktTestU32 $bytes (84 + 4) 512
    Set-TraktTestU32 $bytes (84 + 12) ($layout.FlashOrigin + 224)
    # Main BSS and retained RAM use separate program headers.
    Set-TraktTestU32 $bytes (116 + 20) 6528
    Set-TraktTestU16 $bytes 44 4

    Set-TraktTestU32 $bytes 148 1
    Set-TraktTestU32 $bytes 152 576
    Set-TraktTestU32 $bytes 156 $layout.RetainedOrigin
    Set-TraktTestU32 $bytes 160 $layout.RetainedOrigin
    Set-TraktTestU32 $bytes 164 0
    Set-TraktTestU32 $bytes 168 $layout.RetainedSize
    Set-TraktTestU32 $bytes 172 6
    Set-TraktTestU32 $bytes 176 4

    Set-TraktTestU32 $bytes 256 $layout.StackTop
    for ($index = 1; $index -lt 48; $index++) {
        $value = if ($index -in 4, 5, 6, 7, 8, 9, 10, 12, 13, 46) { 0 } else { $entry }
        Set-TraktTestU32 $bytes (256 + $index * 4) $value
    }
    Set-TraktTestU32 $bytes (256 + 60) ($entry + 2)
    $bytes[448] = 254
    $bytes[449] = 231
    Set-TraktTestU32 $bytes 512 0x13579BDF

    $elf = Read-TraktElf -Bytes $bytes
    $elf.Sections = @(
        [pscustomobject]@{
            Name = '.vector_table'; Type = 1; Flags = 2
            Address = $layout.FlashOrigin; Size = 192
        },
        [pscustomobject]@{
            Name = '.text'; Type = 1; Flags = 6
            Address = $layout.FlashOrigin + 192; Size = 16
        },
        [pscustomobject]@{
            Name = '.data'; Type = 1; Flags = 3
            Address = $layout.RamOrigin; Size = 4
        },
        [pscustomobject]@{
            Name = '.bss'; Type = 8; Flags = 3
            Address = $layout.RamOrigin + 4; Size = 6528
        },
        [pscustomobject]@{
            Name = '.noinit'; Type = 8; Flags = 3
            Address = $layout.RetainedOrigin; Size = 64
        }
    )
    $elf.Symbols = @(
        [pscustomobject]@{
            Name = 'TRAKT_DATA_CHECK'; Size = 4; Kind = 'object'
            Address = $layout.RamOrigin; Section = '.data'
        },
        [pscustomobject]@{
            Name = 'TRAKT_BSS_CHECK'; Size = 4; Kind = 'object'
            Address = $layout.RamOrigin + 4; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_BOOT'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 8; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_FAULT'; Size = 64; Kind = 'object'
            Address = $layout.RetainedOrigin; Section = '.noinit'
        },
        [pscustomobject]@{
            Name = 'TRAKT_TICKS'; Size = 4; Kind = 'object'
            Address = $layout.RamOrigin + 72; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_RUNTIME'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 76; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_FRAME'; Size = 1024; Kind = 'object'
            Address = $layout.RamOrigin + 140; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_DISPLAY'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 1164; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_BENCH'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 1228; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_INPUT'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 1292; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_ADC'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 1356; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_CALIBRATION'; Size = 512; Kind = 'object'
            Address = $layout.RamOrigin + 1420; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_STORAGE'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 1932; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_RECEIVER'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 1996; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_CHANNELS'; Size = 320; Kind = 'object'
            Address = $layout.RamOrigin + 2060; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_SQUELCH'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 2508; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_CONTROL'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 2444; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_JOURNAL_WORK'; Size = 1024; Kind = 'object'
            Address = $layout.RamOrigin + 2380; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_JOURNAL'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 3660; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_TRANSMITTER'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 3596; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_TX_LIMIT'; Size = 16; Kind = 'object'
            Address = $layout.RamOrigin + 3532; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_STACK'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 3804; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_LAST_FAULT'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 3740; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_TIMING'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 3676; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_CAPTURE'; Size = 1044; Kind = 'object'
            Address = $layout.RamOrigin + 3900; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_UART_RX'; Size = 256; Kind = 'object'
            Address = $layout.RamOrigin + 4944; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_SERIAL'; Size = 64; Kind = 'object'
            Address = $layout.RamOrigin + 5200; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_CLEAN_SETTINGS'; Size = 592; Kind = 'object'
            Address = $layout.RamOrigin + 5264; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_SCOPE'; Size = 256; Kind = 'object'
            Address = $layout.RamOrigin + 5856; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_CHANNEL_RX'; Size = 324; Kind = 'object'
            Address = $layout.RamOrigin + 6112; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'TRAKT_TX_METER'; Size = 96; Kind = 'object'
            Address = $layout.RamOrigin + 6436; Section = '.bss'
        },
        [pscustomobject]@{
            Name = 'SysTick_Handler'; Size = 2; Kind = 'function'
            Address = $entry + 2; Section = '.text'
        },
        [pscustomobject]@{
            Name = 'Reset_Handler'; Size = 16; Kind = 'function'
            Address = $entry; Section = '.text'
        }
    )
    # Fixture segment extents are synthetic, not predicted build sizes.
    # Symbol presence follows the requested named composition.
    $features = (Get-TraktVariant $Variant).Features
    $absent = @()
    if ($features -cnotcontains 'debug-tools') {
        $absent += @(Get-TraktOptionalDiagnosticRecords)
    }
    if ($features -cnotcontains 'spectrum') {
        $absent += 'TRAKT_SCOPE'
    }
    if ($features -cnotcontains 'uart') {
        $absent += @(Get-TraktUartRecords)
    }
    $elf.Symbols = @($elf.Symbols | Where-Object {
        $_.Name -cnotin $absent
    })
    return $elf
}

function Invoke-TraktStartupTests {
    $passed = 0
    $failures = [System.Collections.Generic.List[string]]::new()

    try {
        $elf = New-TraktStartupFixture -Variant debug
        Assert-TraktStartupImage -Elf $elf -Variant debug
        if ($elf.Memory.RetainedRamBytes -ne 64 -or
            $elf.Memory.MainStackBudgetBytes -ne 7168 -or
            $elf.Memory.FaultStackBudgetBytes -ne 1024) {
            throw 'Incorrect retained RAM or stack accounting.'
        }
        $passed++
    } catch { $failures.Add("FAIL: startup fixture: $($_.Exception.Message)") }

    $mutations = [ordered]@{
        'Short vector section' = {
            param($elf)
            $elf.Sections[0].Size = 8
        }
        'Even exception vector' = {
            param($elf)
            Set-TraktTestU32 $elf.Image 8 ($elf.Entry - 1)
        }
        'Handler outside code' = {
            param($elf)
            Set-TraktTestU32 $elf.Image 8 ((Get-TraktLayout).FlashOrigin + 1)
        }
        'Reserved core vector' = {
            param($elf)
            Set-TraktTestU32 $elf.Image 16 $elf.Entry
        }
        'Reserved IRQ vector' = {
            param($elf)
            Set-TraktTestU32 $elf.Image (46 * 4) $elf.Entry
        }
        'Missing USB vector' = {
            param($elf)
            Set-TraktTestU32 $elf.Image (47 * 4) 0
        }
        'Initialized fault section' = {
            param($elf)
            $elf.Sections[4].Type = 1
        }
        'Wrong fault record size' = {
            param($elf)
            $elf.Sections[4].Size = 60
        }
        'Missing boot record' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_BOOT' })
        }
        'Wrong SysTick routing' = {
            param($elf)
            Set-TraktTestU32 $elf.Image 60 $elf.Entry
        }
        'Missing tick counter' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_TICKS' })
        }
        'Wrong runtime record size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_RUNTIME' }).Size = 60
        }
        'Missing framebuffer' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_FRAME' })
        }
        'Wrong framebuffer size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_FRAME' }).Size = 1023
        }
        'Initialized display record' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_DISPLAY' }).Section = '.data'
        }
        'Missing bench record' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_BENCH' })
        }
        'Wrong bench record size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_BENCH' }).Size = 60
        }
        'Missing input record' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_INPUT' })
        }
        'Wrong ADC record size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_ADC' }).Size = 60
        }
        'Initialized input record' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_INPUT' }).Section = '.data'
        }
        'Missing calibration buffer' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_CALIBRATION' })
        }
        'Wrong calibration buffer size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_CALIBRATION' }).Size = 511
        }
        'Initialized storage record' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_STORAGE' }).Section = '.data'
        }
        'Missing receiver record' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_RECEIVER' })
        }
        'Wrong receiver record size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_RECEIVER' }).Size = 60
        }
        'Initialized receiver record' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_RECEIVER' }).Section = '.data'
        }
        'Missing squelch record' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_SQUELCH' })
        }
        'Wrong channel bank size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_CHANNELS' }).Size = 319
        }
        'Initialized control record' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_CONTROL' }).Section = '.data'
        }
        'Missing journal workspace' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_JOURNAL_WORK' })
        }
        'Wrong journal workspace size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_JOURNAL_WORK' }).Size = 1023
        }
        'Initialized journal diagnostic' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_JOURNAL' }).Section = '.data'
        }
        'Missing TX record' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_TRANSMITTER' })
        }
        'Wrong TX guard size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_TX_LIMIT' }).Size = 12
        }
        'Initialized TX guard' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_TX_LIMIT' }).Section = '.data'
        }
        'Missing stack observer' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_STACK' })
        }
        'Wrong previous-fault size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_LAST_FAULT' }).Size = 60
        }
        'Initialized stack observer' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_STACK' }).Section = '.data'
        }
        'Retained record overlaps the main stack' = {
            param($elf)
            $section = $elf.Sections | Where-Object { $_.Name -eq '.noinit' }
            $section.Address = (Get-TraktLayout).StackTop - 64
        }
        'Retained segment has an initializer' = {
            param($elf)
            $segment = $elf.Segments | Where-Object { $_.Region -eq 'Retained' }
            $segment.FileSize = 4
        }
        'Missing isolated retained segment' = {
            param($elf)
            $elf.Segments = @($elf.Segments | Where-Object { $_.Region -ne 'Retained' })
        }
        'Missing timing record' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_TIMING' })
        }
        'Wrong timing record size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_TIMING' }).Size = 60
        }
        'Initialized timing record' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_TIMING' }).Section = '.data'
        }
        'Missing capture buffer' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_CAPTURE' })
        }
        'Wrong capture buffer size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_CAPTURE' }).Size = 1043
        }
        'Initialized capture buffer' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_CAPTURE' }).Section = '.data'
        }
        'Missing UART ring' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_UART_RX' })
        }
        'Wrong UART ring size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_UART_RX' }).Size = 255
        }
        'Initialized serial diagnostic' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_SERIAL' }).Section = '.data'
        }
        'Missing settings baseline' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_CLEAN_SETTINGS' })
        }
        'Wrong settings baseline size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_CLEAN_SETTINGS' }).Size = 447
        }
        'Initialized settings baseline' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_CLEAN_SETTINGS' }).Section = '.data'
        }
        'Missing scope samples' = {
            param($elf)
            $elf.Symbols = @($elf.Symbols | Where-Object { $_.Name -ne 'TRAKT_SCOPE' })
        }
        'Wrong scope sample size' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_SCOPE' }).Size = 255
        }
        'Initialized channel import buffer' = {
            param($elf)
            ($elf.Symbols | Where-Object { $_.Name -eq 'TRAKT_CHANNEL_RX' }).Section = '.data'
        }
        'Wrong reset symbol' = {
            param($elf)
            $elf.Symbols[-1].Address += 2
        }
    }

    foreach ($name in $mutations.Keys) {
        try {
            $variant = if ($name -in @(
                'Missing scope samples', 'Wrong scope sample size'
            )) {
                'full'
            } else {
                'debug'
            }
            $elf = New-TraktStartupFixture -Variant $variant
            & $mutations[$name] $elf
            $rejected = $false
            try {
                Assert-TraktStartupImage -Elf $elf -Variant $variant
            } catch {
                $rejected = $true
            }
            if ($rejected) { $passed++ }
            else { $failures.Add("FAIL: accepted invalid startup image: $name") }
        } catch { $failures.Add("FAIL: startup test $name`: $($_.Exception.Message)") }
    }

    return (New-TraktResult -ExitCode ([int]($failures.Count -gt 0)) `
        -Passed $passed -Failed $failures.Count -Output (
            @($failures.ToArray()) + @(
                "Startup structure checks: $passed passed, $($failures.Count) failed."
            )
        ))
}