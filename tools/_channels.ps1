#Requires -Version 7.3
# Checked channel JSON exchange over the existing UART connection.

function Get-TraktChannelInteger {
    param([object]$Value, [long]$Maximum, [string]$Name)

    if ($null -eq $Value -or $Value.GetType() -notin @(
        [byte], [sbyte], [int16], [uint16], [int], [uint32], [long], [ulong]
    )) {
        throw "$Name must be an integer, not a string or floating-point value."
    }
    if ($Value -lt 0 -or $Value -gt $Maximum) {
        throw "$Name is outside its supported range."
    }
    return [long]$Value
}

function Test-TraktChannelFrequency {
    param([long]$Value)

    return $Value % 10 -eq 0 -and (
        ($Value -ge 136000000 -and $Value -le 174000000) -or
        ($Value -ge 400000000 -and $Value -le 470000000)
    )
}

# RX admission is independent of transmit-route representation.
function Test-TraktReceiveFrequency {
    param([long]$Value)

    return $Value % 10 -eq 0 -and (
        ($Value -ge 50000000 -and $Value -lt 630000000) -or
        ($Value -ge 840000000 -and $Value -le 1300000000)
    )
}

function Test-TraktLowerReceiveFrequency {
    param([long]$Value)

    return $Value % 10 -eq 0 -and
        $Value -ge 50000000 -and $Value -lt 630000000
}

# Schema seven keeps the exact receive intervals of journal format sixteen.
function Test-TraktAmFrequency {
    param([long]$Value)

    return $Value % 10 -eq 0 -and (
        ($Value -ge 108000000 -and $Value -le 174000000) -or
        ($Value -ge 400000000 -and $Value -le 470000000)
    )
}

function Test-TraktChannelCtcss {
    param([long]$Value)

    return $Value -in @(
        0,
        670,693,719,744,770,797,825,854,885,915,
        948,974,1000,1035,1072,1109,1148,1188,1230,1273,
        1318,1365,1413,1462,1514,1567,1598,1622,1655,1679,
        1713,1738,1773,1799,1835,1862,1899,1928,1966,1995,
        2035,2065,2107,2181,2257,2291,2336,2418,2503,2541
    )
}

function Get-TraktDcsCodes {
    $source = [System.IO.File]::ReadAllText(
        (Join-Path $global:TraktRoot 'src/dcs.rs')
    )
    $catalog = [regex]::Match(
        $source, '(?s)const CODES: \[u16; 104\] = \[(?<body>.*?)\];'
    )
    if (-not $catalog.Success) { throw 'DCS catalog was not found.' }

    $codes = @([regex]::Matches(
        $catalog.Groups['body'].Value, '0o([0-7]{3})'
    ) | ForEach-Object { $_.Groups[1].Value })

    if ($codes.Count -ne 104 -or
        @($codes | Sort-Object -Unique).Count -ne 104) {
        throw 'DCS catalog is malformed.'
    }
    return $codes
}

function Get-TraktSignalWord {
    param([object]$Signal)

    if ($null -eq $Signal -or $Signal.type -isnot [string]) {
        throw 'Signal selection requires an explicit type.'
    }

    $fields = switch -CaseSensitive ($Signal.type) {
        'off' { @('type') }
        'ctcss' { @('type', 'tenthsHz') }
        'dcs' { @('type', 'code', 'polarity') }
        default { throw 'Signal type must be off, ctcss, or dcs.' }
    }
    foreach ($property in $Signal.PSObject.Properties) {
        if ($property.Name -cnotin $fields) {
            throw "Unexpected signal field: $($property.Name)"
        }
    }

    switch -CaseSensitive ($Signal.type) {
        'off' { return [uint16]0 }
        'ctcss' {
            $value = Get-TraktChannelInteger $Signal.tenthsHz 2541 'signal.tenthsHz'
            if ($value -eq 0 -or -not (Test-TraktChannelCtcss $value)) {
                throw 'CTCSS requires an admitted nonzero frequency.'
            }
            return [uint16]$value
        }
        'dcs' {
            if ($Signal.code -isnot [string] -or
                $Signal.code -cnotin @(Get-TraktDcsCodes) -or
                $Signal.polarity -cnotin 'N','I') {
                throw 'DCS requires a catalogued three-digit octal code and polarity N or I.'
            }
            $code = [System.Convert]::ToInt32($Signal.code, 8)
            $tag = if ($Signal.polarity -ceq 'I') { 0xC000 } else { 0x8000 }
            return [uint16]($tag -bor $code)
        }
    }
}

function ConvertFrom-TraktSignalWord {
    param([uint16]$Word)

    if ($Word -eq 0) {
        return [pscustomobject]@{ type = 'off' }
    }
    if (Test-TraktChannelCtcss $Word) {
        return [pscustomobject]@{ type = 'ctcss'; tenthsHz = [long]$Word }
    }
    if (($Word -band 0xBE00) -ne 0x8000) {
        throw 'Reserved signal bits are nonzero.'
    }

    $signal = [pscustomobject]@{
        type = 'dcs'
        code = [System.Convert]::ToString(($Word -band 511), 8).PadLeft(3, '0')
        polarity = if ($Word -band 0x4000) { 'I' } else { 'N' }
    }
    if ((Get-TraktSignalWord $signal) -ne $Word) {
        throw 'Noncanonical signal encoding.'
    }
    return $signal
}

function Get-TraktChannelRouteWord {
    param([object]$Route)

    if ($null -eq $Route -or $Route.mode -isnot [string]) {
        throw 'Schema two requires transmit.mode and transmit.valueHz.'
    }
    $value = Get-TraktChannelInteger $Route.valueHz 470000000 'transmit.valueHz'
    $mode = switch -CaseSensitive ($Route.mode) {
        'simplex' { 0 }
        'plus' { 1 }
        'minus' { 2 }
        'split' { 3 }
        'rx-only' { 4 }
        default { throw 'Unknown transmit mode.' }
    }

    if ($mode -in 0,4) {
        if ($value -ne 0) { throw 'Simplex and RX-only require valueHz zero.' }
    } elseif ($mode -in 1,2) {
        if ($value -eq 0 -or $value -gt 70000000 -or $value % 10 -ne 0) {
            throw 'Offset requires 10..70000000 Hz on the ten-hertz grid.'
        }
    } elseif (-not (Test-TraktChannelFrequency $value)) {
        throw 'Split frequency is outside the supported bands.'
    }

    return [uint32](([long]$mode -shl 26) -bor [long]($value / 10))
}

function ConvertFrom-TraktChannelRouteWord {
    param([uint32]$Word)

    if (($Word -band 0xE0000000L) -ne 0) { throw 'Reserved route bits are nonzero.' }
    $mode = [int](($Word -shr 26) -band 7)
    if ($mode -gt 4) { throw 'Unknown packed transmit route.' }

    $route = [pscustomobject]@{
        mode = @('simplex','plus','minus','split','rx-only')[$mode]
        valueHz = [long]($Word -band 0x03FFFFFFL) * 10
    }
    if ((Get-TraktChannelRouteWord $route) -ne $Word) {
        throw 'Noncanonical route encoding.'
    }
    return $route
}

function Receive-TraktChannelReply {
    param([object]$Port, [string]$Magic, [string]$Retry = '')

    $received = [System.Collections.Generic.List[byte]]::new()
    [byte[]]$buffer = [byte[]]::new(2048)
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $retryAt = 0L

    while ($watch.Elapsed.TotalSeconds -lt 10) {
        if ($Retry -and $watch.ElapsedMilliseconds - $retryAt -ge 1000) {
            $Port.Write($Retry)
            $retryAt = $watch.ElapsedMilliseconds
        }

        if ($Port.BytesToRead -gt 0) {
            $count = $Port.Read($buffer, 0, [math]::Min($buffer.Length, $Port.BytesToRead))
            for ($index = 0; $index -lt $count; $index++) { $received.Add($buffer[$index]) }
            $reply = Find-TraktChannelReply $received.ToArray() $Magic
            if ($null -ne $reply) { return $reply }
            if ($received.Count -gt 8192) {
                $received.RemoveRange(0, $received.Count - 2048)
            }
        } else {
            Start-Sleep -Milliseconds 20
        }
    }

    throw "No checked $Magic response; received $($received.Count) bytes."
}

function ConvertFrom-TraktChannelBytes {
    param([byte[]]$Bytes, [ValidateSet(1,2,3,4,5,6,7,8,9)][int]$Version = 9)

    if ($Bytes.Length -ne 320) { throw 'Channel payload must contain 320 bytes.' }
    $channels = [System.Collections.Generic.List[object]]::new()

    for ($slot = 0; $slot -lt 16; $slot++) {
        $offset = $slot * 20
        $empty = $true
        for ($index = 0; $index -lt 20; $index++) {
            $empty = $empty -and $Bytes[$offset + $index] -eq 0
        }
        if ($empty) { continue }

        $first = [System.BitConverter]::ToUInt32($Bytes, $offset)
        $second = [System.BitConverter]::ToUInt32($Bytes, $offset + 4)
        $third = [System.BitConverter]::ToUInt32($Bytes, $offset + 8)

        $am = $false
        if ($Version -eq 1) {
            [long]$frequency = $first
            [long]$step = $second
            [long]$width = $third
            $route = ConvertFrom-TraktChannelRouteWord 0
        } else {
            $reserved = if ($Version -ge 9) {
                0x80000000L
            } elseif ($Version -ge 7) {
                0xC0000000L
            } else {
                0xE0000000L
            }
            if (($first -band $reserved) -ne 0 -or
                ($Version -eq 2 -and $third -ne 0) -or
                ($Version -eq 3 -and ($third -band 65535) -ne 0) -or
                ($Version -ge 3 -and $Version -lt 6 -and
                    -not (Test-TraktChannelCtcss ([long]($third -band 65535)))) -or
                ($Version -lt 5 -and
                    -not (Test-TraktChannelCtcss ([long]($third -shr 16))))) {
                throw "Reserved channel fields are nonzero: $($slot + 1)"
            }
            [long]$units = ($first -band 0x03FFFFFFL) -bor
                (($first -shr 4) -band 0x04000000L)
            [long]$frequency = $units * 10
            [long]$width = ($first -shr 26) -band 1
            $am = ($first -band 0x20000000L) -ne 0
            $stepIndex = ($first -shr 27) -band 3
            if ($stepIndex -eq 3) { throw 'Invalid packed tuning step.' }
            [long]$step = @(12500,25000,100000)[$stepIndex]
            $route = ConvertFrom-TraktChannelRouteWord $second
        }

        $frequencyValid = if ($Version -ge 9) {
            Test-TraktReceiveFrequency $frequency
        } elseif ($Version -eq 8) {
            Test-TraktLowerReceiveFrequency $frequency
        } elseif ($Version -eq 7) {
            Test-TraktAmFrequency $frequency
        } else {
            Test-TraktChannelFrequency $frequency
        }
        if (-not $frequencyValid -or
            $step -notin 12500,25000,100000 -or $width -gt 1) {
            throw "Invalid channel profile: $($slot + 1)"
        }

        for ($index = $offset + 12; $index -lt $offset + 20; $index++) {
            if ($Bytes[$index] -lt 32 -or $Bytes[$index] -gt 126) {
                throw "Invalid ASCII name: $($slot + 1)"
            }
        }

        $channels.Add([pscustomobject]@{
            slot = $slot + 1
            frequencyHz = $frequency
            stepHz = $step
            mode = if ($am) {
                if ($width -eq 0) { 'AM' } else { 'NAM' }
            } else {
                if ($width -eq 0) { 'FM' } else { 'NFM' }
            }
            name = [System.Text.Encoding]::ASCII.GetString(
                $Bytes, $offset + 12, 8
            ).TrimEnd(' ')
            transmit = $route
            txSignal = ConvertFrom-TraktSignalWord (
                [uint16]$(if ($Version -ge 3) { $third -shr 16 } else { 0 })
            )
            rxSignal = ConvertFrom-TraktSignalWord (
                [uint16]$(if ($Version -ge 4) { $third -band 65535 } else { 0 })
            )
        })
    }

    return [pscustomobject]@{
        schema = 9
        capacity = 16
        channels = $channels.ToArray()
    }
}

function ConvertTo-TraktChannelBytes {
    param([object]$Document)

    $schema = Get-TraktChannelInteger $Document.schema 9 'schema'
    $capacity = Get-TraktChannelInteger $Document.capacity 16 'capacity'
    if ($schema -notin 1,2,3,4,5,6,7,8,9 -or $capacity -ne 16 -or
        $Document.channels -isnot [array]) {
        throw 'Expected schema 1 through 9, capacity 16, and a channels array.'
    }

    $items = @($Document.channels)
    if ($items.Count -gt 16) { throw 'Too many channels.' }
    [byte[]]$bytes = [byte[]]::new(320)
    $used = [System.Collections.Generic.HashSet[int]]::new()

    foreach ($item in $items) {
        $slot = Get-TraktChannelInteger $item.slot 16 'slot'
        $frequency = Get-TraktChannelInteger $item.frequencyHz 1300000000 'frequencyHz'
        $step = Get-TraktChannelInteger $item.stepHz 100000 'stepHz'

        if ($slot -lt 1 -or -not $used.Add([int]$slot)) {
            throw 'Slots must be unique integers from 1 through 16.'
        }
        $frequencyValid = if ($schema -ge 9) {
            Test-TraktReceiveFrequency $frequency
        } elseif ($schema -eq 8) {
            Test-TraktLowerReceiveFrequency $frequency
        } elseif ($schema -eq 7) {
            Test-TraktAmFrequency $frequency
        } else {
            Test-TraktChannelFrequency $frequency
        }
        if (-not $frequencyValid -or
            $step -notin 12500,25000,100000) {
            throw "Invalid receive frequency or step: $slot"
        }
        $modes = if ($schema -ge 7) { @('FM','NFM','AM','NAM') } else { @('FM','NFM') }
        if ($item.mode -cnotin $modes) { throw "Invalid mode: $slot" }
        if ($item.name -isnot [string] -or $item.name.Length -gt 8 -or
            $item.name -match '[^\x20-\x7E]') {
            throw "Name requires zero through eight printable ASCII characters: $slot"
        }

        if ($schema -eq 1) {
            if ($null -ne $item.PSObject.Properties['transmit']) {
                throw 'Schema one cannot contain transmit fields; use schema two.'
            }
            [uint32]$route = 0
        } else {
            [uint32]$route = Get-TraktChannelRouteWord $item.transmit
        }

        [long]$signal = 0
        if ($schema -ge 5) {
            if ($null -ne $item.PSObject.Properties['txCtcssTenths']) {
                throw 'Schema five uses txSignal, not txCtcssTenths.'
            }
            $signal = Get-TraktSignalWord $item.txSignal
        } else {
            if ($null -ne $item.PSObject.Properties['txSignal']) {
                throw 'txSignal requires JSON schema five.'
            }
            if ($schema -ge 3) {
                $signal = Get-TraktChannelInteger $item.txCtcssTenths 2541 'txCtcssTenths'
                if (-not (Test-TraktChannelCtcss $signal)) { throw 'Unsupported TX CTCSS.' }
            } elseif ($null -ne $item.PSObject.Properties['txCtcssTenths']) {
                throw 'TX CTCSS requires JSON schema three or later.'
            }
        }

        [long]$receiveTone = 0
        if ($schema -ge 6) {
            if ($null -ne $item.PSObject.Properties['rxCtcssTenths']) {
                throw 'Schema six uses rxSignal, not rxCtcssTenths.'
            }
            $receiveTone = Get-TraktSignalWord $item.rxSignal
        } else {
            if ($null -ne $item.PSObject.Properties['rxSignal']) {
                throw 'rxSignal requires JSON schema six or later.'
            }
            if ($schema -ge 4) {
                $receiveTone = Get-TraktChannelInteger $item.rxCtcssTenths 2541 'rxCtcssTenths'
                if (-not (Test-TraktChannelCtcss $receiveTone)) {
                    throw 'Unsupported RX CTCSS.'
                }
            } elseif ($null -ne $item.PSObject.Properties['rxCtcssTenths']) {
                throw 'RX CTCSS requires JSON schema four or later.'
            }
        }

        $stepIndex = switch ($step) { 12500 { 0 } 25000 { 1 } 100000 { 2 } }
        [long]$width = [int]($item.mode -cin 'NFM','NAM')
        [long]$am = [int]($item.mode -cin 'AM','NAM')
        [long]$units = $frequency / 10
        [uint32]$first = ($units -band 0x03FFFFFFL) -bor
            (($units -band 0x04000000L) -shl 4) -bor
            ($width -shl 26) -bor ([long]$stepIndex -shl 27) -bor ($am -shl 29)
        $offset = ([int]$slot - 1) * 20

        [System.BitConverter]::GetBytes($first).CopyTo($bytes, $offset)
        [System.BitConverter]::GetBytes($route).CopyTo($bytes, $offset + 4)
        [System.BitConverter]::GetBytes(
            [uint32](($signal -shl 16) -bor $receiveTone)
        ).CopyTo($bytes, $offset + 8)
        [System.Text.Encoding]::ASCII.GetBytes(
            $item.name.PadRight(8)
        ).CopyTo($bytes, $offset + 12)
    }

    $null = ConvertFrom-TraktChannelBytes -Bytes $bytes -Version 9
    return ,$bytes
}

function Find-TraktChannelReply {
    param([byte[]]$Bytes, [string]$Magic)

    if ($Magic -cnotin 'TKCH','TKCA') { throw 'Unsupported channel reply kind.' }
    [byte[]]$magicBytes = [System.Text.Encoding]::ASCII.GetBytes($Magic)

    for ($offset = 0; $offset + 20 -le $Bytes.Length; $offset++) {
        $matched = $true
        for ($index = 0; $index -lt 4; $index++) {
            $matched = $matched -and $Bytes[$offset + $index] -eq $magicBytes[$index]
        }
        if (-not $matched) { continue }

        $version = $Bytes[$offset + 4]
        $length = [System.BitConverter]::ToUInt32($Bytes, $offset + 8)
        if ($Magic -ceq 'TKCH') {
            if ($version -notin 1,2,3,4,5,6,7,8,9 -or $length -ne 320 -or
                $Bytes[$offset + 5] -ne 16 -or $Bytes[$offset + 6] -ne 20 -or
                $Bytes[$offset + 7] -ne 0) { continue }
        } elseif ($version -ne 1 -or $length -ne 0 -or
            $Bytes[$offset + 5] -notin 1,2 -or
            $Bytes[$offset + 6] -ne 0 -or $Bytes[$offset + 7] -ne 0) {
            continue
        }

        if ($offset + 20 + $length -gt $Bytes.Length) { continue }
        [byte[]]$payload = [byte[]]::new([int]$length)
        [System.Array]::Copy($Bytes, $offset + 20, $payload, 0, [int]$length)
        $crc = [System.BitConverter]::ToUInt32($Bytes, $offset + 12)
        if ((Get-TraktCrc32 $payload) -ne $crc) { continue }

        return [pscustomobject]@{
            Payload = $payload
            Status = $Bytes[$offset + 5]
            Version = $version
        }
    }
    return $null
}

function Invoke-TraktChannels {
    param([string[]]$Arguments)

    $port = $null
    try {
        if ($Arguments.Count -eq 2 -and $Arguments[0] -ceq 'validate') {
            $document = [System.IO.File]::ReadAllText(
                (Resolve-Path -LiteralPath $Arguments[1]).Path
            ) | ConvertFrom-Json
            $wire = ConvertTo-TraktChannelBytes $document
            $checked = ConvertFrom-TraktChannelBytes $wire
            return (New-TraktResult -Output @(
                "Valid channels: $($checked.channels.Count) / 16",
                'Input schema accepted; current wire schema is 9.',
                'No radio connection or file modification was performed.'
            ))
        }

        if ($Arguments.Count -ne 3 -or $Arguments[0] -cnotin 'export','import' -or
            $Arguments[1] -notmatch '^COM\d+$') {
            throw 'Usage: channels export|import COMn FILE.json; channels validate FILE.json'
        }

        $operation = $Arguments[0]
        $file = [System.IO.Path]::GetFullPath($Arguments[2])
        if ($operation -ceq 'export' -and (Test-Path -LiteralPath $file)) {
            throw 'Export refuses to overwrite an existing file.'
        }
        if ($operation -ceq 'import') {
            $document = [System.IO.File]::ReadAllText($file) | ConvertFrom-Json
            [byte[]]$wire = ConvertTo-TraktChannelBytes $document
        }

        if (-not ([System.Management.Automation.PSTypeName]'System.IO.Ports.SerialPort').Type) {
            Add-Type -AssemblyName System.IO.Ports
        }
        $port = [System.IO.Ports.SerialPort]::new(
            $Arguments[1], 38400,
            [System.IO.Ports.Parity]::None, 8, [System.IO.Ports.StopBits]::One
        )
        $port.Handshake = [System.IO.Ports.Handshake]::None
        $port.DtrEnable = $false
        $port.RtsEnable = $false
        $port.ReadTimeout = 100
        $port.WriteTimeout = 500
        $port.Open()
        $port.DiscardInBuffer()

        # Export is also the non-mutating capability check before an import.
        # Never send a new binary body to firmware that does not recognize it.
        $port.Write("TKCE`n")
        $reply = Receive-TraktChannelReply -Port $port -Magic TKCH -Retry "TKCE`n"

        if ($operation -ceq 'import') {
            if ($reply.Version -ne 9) {
                throw 'Radio does not export schema 9. Import was not sent; update firmware first.'
            }

            $port.DiscardInBuffer()
            [byte[]]$request = [System.Text.Encoding]::ASCII.GetBytes("TKKI`n") +
                $wire + [System.BitConverter]::GetBytes([uint32](Get-TraktCrc32 $wire))

            for ($offset = 0; $offset -lt $request.Length; $offset += 32) {
                $length = [math]::Min(32, $request.Length - $offset)
                $port.Write($request, $offset, $length)
                Start-Sleep -Milliseconds 30
            }

            # Imports are not retried automatically.
            $reply = Receive-TraktChannelReply -Port $port -Magic TKCA
            if ($reply.Status -ne 1) {
                throw 'Radio rejected channel validation. RAM was not changed.'
            }
            return (New-TraktResult -Output @(
                'Schema-nine channel bank validated and staged on the radio.',
                'Confirm with MENU on the radio, or cancel with EXIT.',
                'This acknowledgement does not mean applied or saved.'
            ))
        }

        $document = ConvertFrom-TraktChannelBytes -Bytes $reply.Payload -Version $reply.Version
        $directory = [System.IO.Path]::GetDirectoryName($file)
        $null = New-Item -ItemType Directory -Path $directory -Force
        $json = ConvertTo-Json -InputObject $document -Depth 6
        $stream = [System.IO.File]::Open(
            $file, [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write, [System.IO.FileShare]::None
        )
        try {
            $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json + "`n")
            $stream.Write($bytes, 0, $bytes.Length)
        } finally {
            $stream.Dispose()
        }

        return (New-TraktResult -Output @(
            "Exported channels: $($document.channels.Count) / 16; JSON schema 9",
            "File: $file"
        ))
    } catch {
        return (New-TraktResult -ExitCode 1 -Output @($_.Exception.Message))
    } finally {
        if ($null -ne $port) {
            if ($port.IsOpen) { $port.Close() }
            $port.Dispose()
        }
    }
}

function Invoke-TraktChannelTests {
    $passed = 0
    $failures = [System.Collections.Generic.List[string]]::new()

    $cases = [ordered]@{
        'Legacy JSON upgrades to explicit simplex' = {
            $doc = [pscustomobject]@{
                schema = 1; capacity = 16
                channels = @([pscustomobject]@{
                    slot = 1; frequencyHz = 145700000; stepHz = 12500
                    mode = 'FM'; name = 'OLD'
                })
            }
            $loaded = ConvertFrom-TraktChannelBytes (ConvertTo-TraktChannelBytes $doc)
            $loaded.schema -eq 9 -and $loaded.channels[0].transmit.mode -ceq 'simplex'
        }
        'Every route survives a full bank roundtrip' = {
            $modes = @('simplex','plus','minus','split','rx-only')
            $values = @(0,600000,600000,433700000,0)
            $items = @(
                for ($slot = 1; $slot -le 16; $slot++) {
                    $route = ($slot - 1) % 5
                    [pscustomobject]@{
                        slot = $slot; frequencyHz = 145700000; stepHz = 12500
                        mode = 'NFM'; name = "CH$slot"
                        transmit = [pscustomobject]@{
                            mode = $modes[$route]; valueHz = $values[$route]
                        }
                    }
                }
            )
            $doc = [pscustomobject]@{ schema = 2; capacity = 16; channels = $items }
            $first = ConvertTo-TraktChannelBytes $doc
            $loaded = ConvertFrom-TraktChannelBytes $first
            $second = ConvertTo-TraktChannelBytes $loaded
            $loaded.channels.Count -eq 16 -and
                [System.Convert]::ToHexString($first) -ceq [System.Convert]::ToHexString($second)
        }
        'Empty bank roundtrip' = {
            $doc = [pscustomobject]@{ schema = 2; capacity = 16; channels = @() }
            (ConvertFrom-TraktChannelBytes (ConvertTo-TraktChannelBytes $doc)).channels.Count -eq 0
        }
        'Noninteger values cannot pass through cast rounding' = {
            foreach ($value in @('1', 1.5, $true, $null)) {
                $rejected = $false
                try { $null = Get-TraktChannelInteger $value 16 'slot' }
                catch { $rejected = $true }
                if (-not $rejected) { throw 'Noninteger accepted.' }
            }
            $true
        }
        'Malformed route encodings are rejected' = {
            [uint32[]]$words = @(
                0x00000001,  # Simplex with a nonzero argument.
                0x04000000,  # Positive offset with zero magnitude.
                0x08000000,  # Negative offset with zero magnitude.
                0x10000001,  # RX-only with a nonzero argument.
                0x14000000,  # Unknown mode five.
                0xE0000000L  # Reserved high bits.
            )

            foreach ($word in $words) {
                $rejected = $false
                try { $null = ConvertFrom-TraktChannelRouteWord $word }
                catch { $rejected = $true }

                if (-not $rejected) {
                    throw ('Malformed route accepted: 0x{0:X8}' -f $word)
                }
            }
            $true
        }
        'RX-only with a zero argument is valid' = {
            $route = ConvertFrom-TraktChannelRouteWord ([uint32]0x10000000)
            $route.mode -ceq 'rx-only' -and
                $route.valueHz -eq 0 -and
                (Get-TraktChannelRouteWord $route) -eq 0x10000000
        }
        'Invalid empty slot is rejected' = {
            $bytes = [byte[]]::new(320)
            $bytes[319] = 32
            try { $null = ConvertFrom-TraktChannelBytes $bytes; $false } catch { $true }
        }
        'Legacy wire imports simplex' = {
            $bytes = [byte[]]::new(320)
            [System.BitConverter]::GetBytes([uint32]145700000).CopyTo($bytes, 0)
            [System.BitConverter]::GetBytes([uint32]12500).CopyTo($bytes, 4)
            [System.Text.Encoding]::ASCII.GetBytes('LEGACY  ').CopyTo($bytes, 12)
            $doc = ConvertFrom-TraktChannelBytes $bytes -Version 1
            $doc.channels[0].frequencyHz -eq 145700000 -and
                $doc.channels[0].transmit.mode -ceq 'simplex'
        }
        'Both reply schemas require complete payload and CRC' = {
            foreach ($version in @(1,2,3,4,5,6,7,8,9)) {
                $bytes = [byte[]]::new(340)
                [System.Text.Encoding]::ASCII.GetBytes('TKCH').CopyTo($bytes, 0)
                $bytes[4] = $version
                $bytes[5] = 16
                $bytes[6] = 20
                [System.BitConverter]::GetBytes([uint32]320).CopyTo($bytes, 8)
                [System.BitConverter]::GetBytes(
                    [uint32](Get-TraktCrc32 ([byte[]]::new(320)))
                ).CopyTo($bytes, 12)
                $reply = Find-TraktChannelReply $bytes TKCH
                if ($null -eq $reply -or $reply.Version -ne $version) {
                    throw 'Valid channel response rejected.'
                }
                if ($null -ne (Find-TraktChannelReply $bytes[0..338] TKCH)) {
                    throw 'Truncated response accepted.'
                }
                $bytes[339] = 1
                if ($null -ne (Find-TraktChannelReply $bytes TKCH)) {
                    throw 'Corrupt response accepted.'
                }
            }
            $true
        }
        'TX CTCSS survives current JSON and wire roundtrip' = {
            $doc = [pscustomobject]@{
                schema = 3; capacity = 16
                channels = @([pscustomobject]@{
                    slot = 1; frequencyHz = 145700000; stepHz = 12500
                    mode = 'FM'; name = 'CTCSS'
                    transmit = [pscustomobject]@{ mode = 'minus'; valueHz = 600000 }
                    txCtcssTenths = 885
                })
            }
            $bytes = ConvertTo-TraktChannelBytes $doc
            $restored = ConvertFrom-TraktChannelBytes $bytes
            if ($restored.channels[0].txSignal.tenthsHz -ne 885) { throw 'Tone lost.' }

            $rejected = $false
            try { $null = ConvertFrom-TraktChannelBytes $bytes -Version 2 }
            catch { $rejected = $true }
            if (-not $rejected) { throw 'Old schema accepted new signaling.' }

            $doc.channels[0].txCtcssTenths = 886
            $rejected = $false
            try { $null = ConvertTo-TraktChannelBytes $doc }
            catch { $rejected = $true }
            $rejected
        }
        'Independent RX and TX tones survive schema four' = {
            $doc = [pscustomobject]@{
                schema = 4; capacity = 16
                channels = @([pscustomobject]@{
                    slot = 1; frequencyHz = 145700000; stepHz = 12500
                    mode = 'FM'; name = 'RX-TX'
                    transmit = [pscustomobject]@{ mode = 'minus'; valueHz = 600000 }
                    txCtcssTenths = 1000
                    rxCtcssTenths = 885
                })
            }
            $bytes = ConvertTo-TraktChannelBytes $doc
            $loaded = ConvertFrom-TraktChannelBytes $bytes
            if ($loaded.channels[0].txSignal.tenthsHz -ne 1000 -or
                $loaded.channels[0].rxSignal.tenthsHz -ne 885) {
                throw 'Directional tone values were changed.'
            }
            $rejected = $false
            try { $null = ConvertFrom-TraktChannelBytes $bytes -Version 3 }
            catch { $rejected = $true }
            if (-not $rejected) { throw 'Schema three accepted RX signaling.' }

            $doc.channels[0].rxCtcssTenths = 886
            $rejected = $false
            try { $null = ConvertTo-TraktChannelBytes $doc }
            catch { $rejected = $true }
            $rejected
        }
        'DCS selection preserves octal code and polarity' = {
            foreach ($polarity in @('N','I')) {
                $doc = [pscustomobject]@{
                    schema = 5; capacity = 16
                    channels = @([pscustomobject]@{
                        slot = 1; frequencyHz = 433700000; stepHz = 12500
                        mode = 'FM'; name = 'DCS'
                        transmit = [pscustomobject]@{ mode = 'simplex'; valueHz = 0 }
                        txSignal = [pscustomobject]@{
                            type = 'dcs'; code = '023'; polarity = $polarity
                        }
                        rxCtcssTenths = 885
                    })
                }
                $bytes = ConvertTo-TraktChannelBytes $doc
                $restored = ConvertFrom-TraktChannelBytes $bytes
                $signal = $restored.channels[0].txSignal
                if ($signal.type -cne 'dcs' -or $signal.code -cne '023' -or
                    $signal.polarity -cne $polarity -or
                    $restored.channels[0].rxSignal.tenthsHz -ne 885) {
                    throw 'DCS channel fields changed during roundtrip.'
                }

                $rejected = $false
                try { $null = ConvertFrom-TraktChannelBytes $bytes -Version 4 }
                catch { $rejected = $true }
                if (-not $rejected) { throw 'Schema four accepted DCS.' }
            }
            $true
        }
        'DCS tags and malformed codes are checked' = {
            $normal = [pscustomobject]@{ type = 'dcs'; code = '023'; polarity = 'N' }
            if ((Get-TraktSignalWord $normal) -ne 0x8013) {
                throw 'Normal DCS tag mismatch.'
            }
            $normal.polarity = 'I'
            if ((Get-TraktSignalWord $normal) -ne 0xC013) {
                throw 'Inverted DCS tag mismatch.'
            }

            foreach ($word in @(0x4013,0x8213,0x8001,0xFFFF)) {
                $rejected = $false
                try { $null = ConvertFrom-TraktSignalWord ([uint16]$word) }
                catch { $rejected = $true }
                if (-not $rejected) { throw 'Malformed TX signal accepted.' }
            }
            foreach ($code in @('23','028','777',23)) {
                $normal.code = $code
                $rejected = $false
                try { $null = Get-TraktSignalWord $normal }
                catch { $rejected = $true }
                if (-not $rejected) { throw 'Malformed octal code accepted.' }
            }
            $true
        }
        'TX families cannot be specified together' = {
            $mixed = [pscustomobject]@{
                type = 'dcs'; code = '023'; polarity = 'N'; tenthsHz = 1000
            }
            try { $null = Get-TraktSignalWord $mixed; $false } catch { $true }
        }
        'RX and TX DCS remain independent in schema six' = {
            foreach ($polarity in @('N','I')) {
                $doc = [pscustomobject]@{
                    schema = 6; capacity = 16
                    channels = @([pscustomobject]@{
                        slot = 1; frequencyHz = 433700000; stepHz = 12500
                        mode = 'NFM'; name = 'TWO DCS'
                        transmit = [pscustomobject]@{ mode = 'simplex'; valueHz = 0 }
                        txSignal = [pscustomobject]@{
                            type = 'dcs'; code = '125'; polarity = 'I'
                        }
                        rxSignal = [pscustomobject]@{
                            type = 'dcs'; code = '023'; polarity = $polarity
                        }
                    })
                }
                $wire = ConvertTo-TraktChannelBytes $doc
                $loaded = ConvertFrom-TraktChannelBytes $wire
                $channel = $loaded.channels[0]
                if ($channel.rxSignal.code -cne '023' -or
                    $channel.rxSignal.polarity -cne $polarity -or
                    $channel.txSignal.code -cne '125' -or
                    $channel.txSignal.polarity -cne 'I') {
                    throw 'Directional DCS fields changed.'
                }

                $again = ConvertTo-TraktChannelBytes $loaded
                if ([System.Convert]::ToHexString($wire) -cne
                    [System.Convert]::ToHexString($again)) {
                    throw 'Schema-six wire roundtrip changed bytes.'
                }

                $rejected = $false
                try { $null = ConvertFrom-TraktChannelBytes $wire -Version 5 }
                catch { $rejected = $true }
                if (-not $rejected) { throw 'Schema five accepted digital RX.' }

                $doc.channels[0].rxSignal.code = '777'
                $rejected = $false
                try { $null = ConvertTo-TraktChannelBytes $doc }
                catch { $rejected = $true }
                if (-not $rejected) { throw 'Invalid RX DCS accepted.' }
            }
            $true
        }
        'AM and aviation RX require schema seven' = {
            foreach ($mode in @('AM','NAM')) {
                $doc = [pscustomobject]@{
                    schema = 7; capacity = 16
                    channels = @([pscustomobject]@{
                        slot = 1; frequencyHz = 118025000; stepHz = 25000
                        mode = $mode; name = 'AIR'
                        transmit = [pscustomobject]@{ mode = 'rx-only'; valueHz = 0 }
                        txSignal = [pscustomobject]@{ type = 'off' }
                        rxSignal = [pscustomobject]@{
                            type = 'ctcss'; tenthsHz = 885
                        }
                    })
                }
                $wire = ConvertTo-TraktChannelBytes $doc
                $loaded = ConvertFrom-TraktChannelBytes $wire
                if ($loaded.schema -ne 9 -or
                    $loaded.channels[0].mode -cne $mode -or
                    $loaded.channels[0].frequencyHz -ne 118025000 -or
                    $loaded.channels[0].rxSignal.tenthsHz -ne 885) {
                    throw 'AM channel roundtrip changed fields.'
                }
                $again = ConvertTo-TraktChannelBytes $loaded
                if ([System.Convert]::ToHexString($wire) -cne
                    [System.Convert]::ToHexString($again)) {
                    throw 'AM wire roundtrip changed bytes.'
                }

                $rejected = $false
                try { $null = ConvertFrom-TraktChannelBytes $wire -Version 6 }
                catch { $rejected = $true }
                if (-not $rejected) { throw 'Schema six accepted AM.' }

                $doc.schema = 6
                $rejected = $false
                try { $null = ConvertTo-TraktChannelBytes $doc }
                catch { $rejected = $true }
                if (-not $rejected) { throw 'Old JSON accepted extended fields.' }
            }
            (Test-TraktReceiveFrequency 108000000) -and
                -not (Test-TraktReceiveFrequency 49999990) -and
                -not (Test-TraktChannelFrequency 118025000)
        }
        'AM on an existing band still needs schema seven' = {
            $doc = [pscustomobject]@{
                schema = 7; capacity = 16
                channels = @([pscustomobject]@{
                    slot = 1; frequencyHz = 433500000; stepHz = 12500
                    mode = 'AM'; name = 'AM'
                    transmit = [pscustomobject]@{ mode = 'simplex'; valueHz = 0 }
                    txSignal = [pscustomobject]@{ type = 'off' }
                    rxSignal = [pscustomobject]@{ type = 'off' }
                })
            }
            $wire = ConvertTo-TraktChannelBytes $doc
            $rejected = $false
            try { $null = ConvertFrom-TraktChannelBytes $wire -Version 6 }
            catch { $rejected = $true }
            if (-not $rejected) { throw 'Historical wire accepted AM bit.' }

            $doc.channels[0].mode = 'FM'
            $wire = ConvertTo-TraktChannelBytes $doc
            (ConvertFrom-TraktChannelBytes $wire -Version 6).channels[0].mode -ceq 'FM'
        }
        'Extended receive profiles require schema eight' = {
            foreach ($frequency in @(50000000,250000000,350000000,629999990)) {
                foreach ($mode in @('FM','NFM','AM','NAM')) {
                    $doc = [pscustomobject]@{
                        schema = 8; capacity = 16
                        channels = @([pscustomobject]@{
                            slot = 16; frequencyHz = $frequency; stepHz = 12500
                            mode = $mode; name = 'WIDE RX'
                            transmit = [pscustomobject]@{
                                mode = 'rx-only'; valueHz = 0
                            }
                            txSignal = [pscustomobject]@{ type = 'off' }
                            rxSignal = [pscustomobject]@{
                                type = 'ctcss'; tenthsHz = 885
                            }
                        })
                    }
                    $bytes = ConvertTo-TraktChannelBytes $doc
                    $loaded = ConvertFrom-TraktChannelBytes $bytes
                    if ($loaded.schema -ne 9 -or
                        $loaded.channels[0].frequencyHz -ne $frequency -or
                        $loaded.channels[0].mode -cne $mode) {
                        throw 'Extended receive fields changed.'
                    }
                    $again = ConvertTo-TraktChannelBytes $loaded
                    if ([System.Convert]::ToHexString($bytes) -cne
                        [System.Convert]::ToHexString($again)) {
                        throw 'Extended wire roundtrip changed bytes.'
                    }

                    $rejected = $false
                    try { $null = ConvertFrom-TraktChannelBytes $bytes -Version 7 }
                    catch { $rejected = $true }
                    if (-not $rejected) {
                        throw 'Schema seven accepted extended reception.'
                    }

                    $doc.schema = 7
                    $rejected = $false
                    try { $null = ConvertTo-TraktChannelBytes $doc }
                    catch { $rejected = $true }
                    if (-not $rejected) {
                        throw 'Schema-seven JSON accepted extended reception.'
                    }
                }
            }
            $true
        }
        'Extended RX cannot extend split TX encoding' = {
            foreach ($frequency in @(50000000,250000000,350000000,629999990)) {
                $route = [pscustomobject]@{
                    mode = 'split'; valueHz = $frequency
                }
                $rejected = $false
                try { $null = Get-TraktChannelRouteWord $route }
                catch { $rejected = $true }
                if (-not $rejected) { throw 'RX extension leaked into TX route.' }
            }
            -not (Test-TraktReceiveFrequency 630000000) -and
                -not (Test-TraktReceiveFrequency 839999990) -and
                -not (Test-TraktReceiveFrequency 50000001)
        }
        'Upper RX uses a distinct packed frequency bit' = {
            foreach ($frequency in @(840000000,1000000000,1300000000)) {
                foreach ($mode in @('FM','NFM','AM','NAM')) {
                    $doc = [pscustomobject]@{
                        schema = 9; capacity = 16
                        channels = @([pscustomobject]@{
                            slot = 1; frequencyHz = $frequency; stepHz = 12500
                            mode = $mode; name = 'UPPER RX'
                            transmit = [pscustomobject]@{
                                mode = 'rx-only'; valueHz = 0
                            }
                            txSignal = [pscustomobject]@{ type = 'off' }
                            rxSignal = [pscustomobject]@{
                                type = 'ctcss'; tenthsHz = 885
                            }
                        })
                    }
                    $bytes = ConvertTo-TraktChannelBytes $doc
                    $first = [System.BitConverter]::ToUInt32($bytes, 0)
                    if (($first -band 0x40000000L) -eq 0) {
                        throw 'Upper frequency bit missing.'
                    }

                    $loaded = ConvertFrom-TraktChannelBytes $bytes
                    if ($loaded.channels[0].frequencyHz -ne $frequency -or
                        $loaded.channels[0].mode -cne $mode) {
                        throw 'Upper RX fields changed.'
                    }
                    $again = ConvertTo-TraktChannelBytes $loaded
                    if ([System.Convert]::ToHexString($bytes) -cne
                        [System.Convert]::ToHexString($again)) {
                        throw 'Upper RX wire roundtrip changed bytes.'
                    }

                    $rejected = $false
                    try { $null = ConvertFrom-TraktChannelBytes $bytes -Version 8 }
                    catch { $rejected = $true }
                    if (-not $rejected) { throw 'Old wire accepted upper RX.' }

                    $doc.schema = 8
                    $rejected = $false
                    try { $null = ConvertTo-TraktChannelBytes $doc }
                    catch { $rejected = $true }
                    if (-not $rejected) { throw 'Old JSON accepted upper RX.' }
                }
            }
            (Test-TraktReceiveFrequency 1300000000) -and
                -not (Test-TraktReceiveFrequency 1300000010)
        }
        'Acknowledgement remains schema one' = {
            [byte[]]$bytes = @(84,75,67,65,1,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0)
            if ($null -eq (Find-TraktChannelReply $bytes TKCA)) { throw 'Ack rejected.' }
            $bytes[12] = 1
            $null -eq (Find-TraktChannelReply $bytes TKCA)
        }
    }

    foreach ($name in $cases.Keys) {
        try {
            if (& $cases[$name]) { $passed++ } else { $failures.Add("FAIL: $name") }
        } catch {
            $failures.Add("FAIL: $name; $($_.Exception.Message)")
        }
    }

    return (New-TraktResult -ExitCode ([int]($failures.Count -gt 0)) `
        -Passed $passed -Failed $failures.Count -Output $failures.ToArray())
}