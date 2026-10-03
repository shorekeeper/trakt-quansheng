#Requires -Version 7.3
# Read-only inspection of a raw calibration block or an explicit dump range.

function Get-TraktCrc32 {
    param([byte[]]$Bytes)

    [long]$crc = 0xFFFFFFFFL
    foreach ($byte in $Bytes) {
        $crc = $crc -bxor $byte
        for ($bit = 0; $bit -lt 8; $bit++) {
            if ($crc -band 1) {
                $crc = ($crc -shr 1) -bxor 0xEDB88320L
            } else {
                $crc = $crc -shr 1
            }
        }
    }
    return [uint32]($crc -bxor 0xFFFFFFFFL)
}

function Get-TraktCalibrationInfo {
    param([byte[]]$Bytes)

    if ($Bytes.Length -ne 512) { throw 'A calibration block must contain exactly 512 bytes.' }
    $present = @($Bytes | Where-Object { $_ -ne 0 }).Count -gt 0 -and
        @($Bytes | Where-Object { $_ -ne 255 }).Count -gt 0

    $battery = @(
        for ($index = 0; $index -lt 6; $index++) {
            [System.BitConverter]::ToUInt16($Bytes, 0x140 + $index * 2)
        }
    )
    $crystal = [System.BitConverter]::ToInt16($Bytes, 0x188)
    $volume = $Bytes[0x18E]
    $dac = $Bytes[0x18F]

    $flags = 0
    if ($present) { $flags = $flags -bor 1 }
    if ($present -and $battery[0] -gt 0 -and
        $battery[0] -lt $battery[1] -and $battery[1] -le 4095) {
        $flags = $flags -bor 2
    }
    if ($present -and $crystal -ge -1000 -and $crystal -le 1000 -and
        $volume -lt 64 -and $dac -lt 16) {
        $flags = $flags -bor 4
    }

    return [pscustomobject]@{
        Crc32 = Get-TraktCrc32 $Bytes
        Flags = $flags
        Battery = $battery
        Crystal = $crystal
        FilterLow = [System.BitConverter]::ToUInt16($Bytes, 0x18A)
        FilterHigh = [System.BitConverter]::ToUInt16($Bytes, 0x18C)
        Volume = $volume
        Dac = $dac
    }
}

function Invoke-TraktCalibration {
    param([string[]]$Arguments)

    if ($Arguments.Count -notin 1, 3 -or
        ($Arguments.Count -eq 3 -and $Arguments[1] -cne '--offset')) {
        return (New-TraktResult -ExitCode 2 -Output @(
            'Usage: calib FILE [--offset DECIMAL_OR_HEX]',
            'Without an offset, FILE must be a raw 512-byte calibration block.'
        ))
    }

    try {
        $file = Get-Item -LiteralPath $Arguments[0] -ErrorAction Stop
        if ($file.PSIsContainer -or $file.Length -gt 16 * 1024 * 1024) {
            throw 'Expected a file no larger than 16 MiB.'
        }
        [long]$offset = 0
        if ($Arguments.Count -eq 3) {
            $value = $Arguments[2]
            if ($value -match '^0[xX][0-9a-fA-F]+$') {
                $offset = [System.Convert]::ToInt64($value.Substring(2), 16)
            } elseif ($value -match '^\d+$') {
                $offset = [long]::Parse($value, [System.Globalization.CultureInfo]::InvariantCulture)
            } else {
                throw 'Offset must be a nonnegative decimal or hexadecimal integer.'
            }
        } elseif ($file.Length -ne 512) {
            throw 'A larger dump requires an explicit offset; its format is not inferred.'
        }

        $bytes = [System.IO.File]::ReadAllBytes($file.FullName)
        Assert-TraktSpan $offset 512 0 $bytes.Length 'calibration dump'
        [byte[]]$block = [byte[]]::new(512)
        [System.Array]::Copy($bytes, [int]$offset, $block, 0, 512)
        $info = Get-TraktCalibrationInfo $block

        return (New-TraktResult -Output @(
            ('Offset       0x{0:X}' -f $offset),
            ('CRC32        {0:X8}' -f $info.Crc32),
            "Flags        $($info.Flags) (plausibility only)",
            "Battery      $($info.Battery -join ', ')",
            "Crystal      $($info.Crystal)",
            "Filters      $($info.FilterLow), $($info.FilterHigh)",
            "Audio gains  $($info.Volume), $($info.Dac)",
            'The file was read only. No values were applied to firmware or hardware.'
        ))
    } catch {
        return (New-TraktResult -ExitCode 1 -Output @($_.Exception.Message))
    }
}

function Invoke-TraktCalibrationTests {
    $passed = 0
    $failures = [System.Collections.Generic.List[string]]::new()

    $cases = [ordered]@{
        'CRC32 known vector' = {
            (Get-TraktCrc32 ([System.Text.Encoding]::ASCII.GetBytes('123456789'))) -eq 0xCBF43926L
        }
        'CRC32 empty vector' = {
            (Get-TraktCrc32 ([byte[]]::new(0))) -eq 0
        }
        'Zero block rejection flags' = {
            (Get-TraktCalibrationInfo ([byte[]]::new(512))).Flags -eq 0
        }
        'Erased block rejection flags' = {
            [byte[]]$bytes = [byte[]]::new(512)
            for ($index = 0; $index -lt 512; $index++) { $bytes[$index] = 255 }
            (Get-TraktCalibrationInfo $bytes).Flags -eq 0
        }
        'Calibration little endian and signed fields' = {
            [byte[]]$bytes = [byte[]]::new(512)
            $bytes[0x140] = 0x6C
            $bytes[0x141] = 7
            $bytes[0x142] = 0xD0
            $bytes[0x143] = 7
            $bytes[0x188] = 0xFB
            $bytes[0x189] = 255
            $bytes[0x18A] = 70
            $bytes[0x18C] = 80
            $bytes[0x18E] = 58
            $bytes[0x18F] = 8
            $info = Get-TraktCalibrationInfo $bytes
            $info.Flags -eq 7 -and $info.Battery[0] -eq 1900 -and
                $info.Battery[1] -eq 2000 -and $info.Crystal -eq -5 -and
                $info.FilterLow -eq 70 -and $info.FilterHigh -eq 80
        }
        'Short calibration rejection' = {
            try {
                $null = Get-TraktCalibrationInfo ([byte[]]::new(511))
                $false
            } catch { $true }
        }
    }

    foreach ($name in $cases.Keys) {
        try {
            if (& $cases[$name]) { $passed++ }
            else { $failures.Add("FAIL: $name") }
        } catch { $failures.Add("FAIL: $name`: $($_.Exception.Message)") }
    }

    return (New-TraktResult -ExitCode ([int]($failures.Count -gt 0)) `
        -Passed $passed -Failed $failures.Count -Output (
            @($failures.ToArray()) + @("Calibration host checks: $passed passed.")
        ))
}