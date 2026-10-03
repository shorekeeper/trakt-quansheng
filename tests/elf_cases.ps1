#Requires -Version 7.3
# Synthetic ELF tests do not execute target code or require Cargo.

function Set-TraktTestU16 {
    param([byte[]]$Bytes, [int]$Offset, [uint16]$Value)
    [System.BitConverter]::GetBytes($Value).CopyTo($Bytes, $Offset)
}

function Set-TraktTestU32 {
    param([byte[]]$Bytes, [int]$Offset, [uint32]$Value)
    [System.BitConverter]::GetBytes($Value).CopyTo($Bytes, $Offset)
}

function New-TraktTestElf {
    $layout = Get-TraktLayout
    [byte[]]$bytes = [byte[]]::new(320)
    $bytes[0] = 127
    $bytes[1] = 69
    $bytes[2] = 76
    $bytes[3] = 70
    $bytes[4] = 1
    $bytes[5] = 1
    $bytes[6] = 1

    Set-TraktTestU16 $bytes 16 2
    Set-TraktTestU16 $bytes 18 40
    Set-TraktTestU32 $bytes 20 1
    Set-TraktTestU32 $bytes 24 ($layout.FlashOrigin + 9)
    Set-TraktTestU32 $bytes 28 52
    Set-TraktTestU32 $bytes 36 0x05000200
    Set-TraktTestU16 $bytes 40 52
    Set-TraktTestU16 $bytes 42 32
    Set-TraktTestU16 $bytes 44 3

    $segments = @(
        @{
            Offset = 0x100; Address = $layout.FlashOrigin
            Load = $layout.FlashOrigin; File = 16; Memory = 16; Flags = 5
        },
        @{
            Offset = 0x120; Address = $layout.RamOrigin
            Load = $layout.FlashOrigin + 32; File = 4; Memory = 4; Flags = 6
        },
        @{
            Offset = 0x124; Address = $layout.RamOrigin + 4
            Load = $layout.RamOrigin + 4; File = 0; Memory = 32; Flags = 6
        }
    )
    for ($i = 0; $i -lt $segments.Count; $i++) {
        $at = 52 + $i * 32
        $segment = $segments[$i]
        Set-TraktTestU32 $bytes $at 1
        Set-TraktTestU32 $bytes ($at + 4) $segment.Offset
        Set-TraktTestU32 $bytes ($at + 8) $segment.Address
        Set-TraktTestU32 $bytes ($at + 12) $segment.Load
        Set-TraktTestU32 $bytes ($at + 16) $segment.File
        Set-TraktTestU32 $bytes ($at + 20) $segment.Memory
        Set-TraktTestU32 $bytes ($at + 24) $segment.Flags
        Set-TraktTestU32 $bytes ($at + 28) 4
    }

    Set-TraktTestU32 $bytes 0x100 $layout.StackTop
    Set-TraktTestU32 $bytes 0x104 ($layout.FlashOrigin + 9)
    $bytes[0x108] = 254
    $bytes[0x109] = 231
    Set-TraktTestU32 $bytes 0x120 0x13579BDF
    return ,$bytes
}

function Invoke-TraktElfTests {
    $passed = 0
    $failures = [System.Collections.Generic.List[string]]::new()
    $layout = Get-TraktLayout

    try {
        $elf = Read-TraktElf -Bytes (New-TraktTestElf)
        if ($elf.Memory.BinBytes -ne 36 -or
            $elf.Memory.FlashLoadBytes -ne 20 -or
            $elf.Memory.FlashGapBytes -ne 16 -or
            $elf.Memory.StaticRamBytes -ne 36 -or
            $elf.Memory.StackBudgetBytes -ne 8192 -or
            (Get-TraktU32 $elf.Image 32) -ne 0x13579BDF) {
            throw 'Unexpected memory accounting or RAM initializer export.'
        }
        foreach ($index in 16..31) {
            if ($elf.Image[$index] -ne 255) { throw 'Image gaps must contain 0xFF.' }
        }
        $passed++
    } catch { $failures.Add("FAIL: valid ELF: $($_.Exception.Message)") }

    $mutations = [ordered]@{
        'ELF magic' = { param($b) $b[0] = 0 }
        'ELF64' = { param($b) $b[4] = 2 }
        'Big endian' = { param($b) $b[5] = 2 }
        'Wrong machine' = { param($b) Set-TraktTestU16 $b 18 62 }
        'Relocatable file' = { param($b) Set-TraktTestU16 $b 16 1 }
        'Wrong EABI' = { param($b) Set-TraktTestU32 $b 36 0 }
        'Program table bounds' = { param($b) Set-TraktTestU32 $b 28 300 }
        'Segment file bounds' = { param($b) Set-TraktTestU32 $b (52 + 4) 319 }
        'File larger than memory' = { param($b) Set-TraktTestU32 $b (52 + 20) 8 }
        'Invalid alignment' = { param($b) Set-TraktTestU32 $b (52 + 28) 3 }
        'Flash below application' = {
            param($b)
            Set-TraktTestU32 $b (84 + 12) ($layout.FlashOrigin - 4)
        }
        'Flash load overlap' = {
            param($b)
            Set-TraktTestU32 $b (84 + 12) ($layout.FlashOrigin + 8)
        }
        'RAM run overlap' = {
            param($b)
            Set-TraktTestU32 $b (116 + 8) $layout.RamOrigin
        }
        'RAM stack collision' = {
            param($b)
            Set-TraktTestU32 $b (116 + 8) (
                $layout.RamOrigin + $layout.RamSize - $layout.StackSize - 4
            )
        }
        'Dynamic segment' = { param($b) Set-TraktTestU32 $b 84 2 }
        'Even reset address' = {
            param($b)
            Set-TraktTestU32 $b 0x104 ($layout.FlashOrigin + 8)
        }
        'Entry outside code' = {
            param($b)
            Set-TraktTestU32 $b 24 ($layout.FlashOrigin + 257)
            Set-TraktTestU32 $b 0x104 ($layout.FlashOrigin + 257)
        }
        'Invalid stack pointer' = {
            param($b)
            Set-TraktTestU32 $b 0x100 $layout.RamOrigin
        }
        'Section table bounds' = {
            param($b)
            Set-TraktTestU32 $b 32 300
            Set-TraktTestU16 $b 46 40
            Set-TraktTestU16 $b 48 2
            Set-TraktTestU16 $b 50 1
        }
    }

    foreach ($name in $mutations.Keys) {
        $bytes = New-TraktTestElf
        & $mutations[$name] $bytes
        $rejected = $false
        try { $null = Read-TraktElf -Bytes $bytes } catch { $rejected = $true }
        if ($rejected) { $passed++ }
        else { $failures.Add("FAIL: accepted invalid ELF: $name") }
    }

    try {
        $rejected = $false
        try { $null = Read-TraktElf -Bytes ([byte[]]::new(40)) } catch { $rejected = $true }
        if (-not $rejected) { throw 'Truncated header accepted.' }
        $passed++
    } catch { $failures.Add("FAIL: truncated header: $($_.Exception.Message)") }

    try {
        $parsed = ConvertFrom-TraktCargo ([pscustomobject]@{
            ExitCode = 0
            Stdout = @(
                '{"reason":"compiler-artifact","target":{"name":"trakt","kind":["bin"]},"executable":"probe"}',
                '{"reason":"build-finished","success":true}'
            )
            Stderr = @()
        })
        if ($parsed.Artifacts.Count -ne 1 -or $parsed.Artifacts[0] -cne 'probe') {
            throw 'Cargo artifact was not decoded.'
        }
        $passed++
    } catch { $failures.Add("FAIL: Cargo JSON: $($_.Exception.Message)") }

    try {
        $rejected = $false
        try {
            $null = ConvertFrom-TraktCargo ([pscustomobject]@{
                ExitCode = 0
                Stdout = @('{"reason":"build-finished","success":false}')
                Stderr = @()
            })
        } catch { $rejected = $true }
        if (-not $rejected) { throw 'Contradictory Cargo result accepted.' }
        $passed++
    } catch { $failures.Add("FAIL: Cargo completion: $($_.Exception.Message)") }

    return (New-TraktResult -ExitCode ([int]($failures.Count -gt 0)) `
        -Passed $passed -Failed $failures.Count -Output (
            @($failures.ToArray()) + @("ELF and Cargo checks: $passed passed, $($failures.Count) failed.")
        ))
}