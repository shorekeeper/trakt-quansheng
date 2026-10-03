#Requires -Version 7.3
# Checked UART capture and grayscale PNG output without drawing libraries.

function Find-TraktFrame {
    param([byte[]]$Bytes)

    for ($offset = 0; $offset + 20 -le $Bytes.Length; $offset++) {
        if ($Bytes[$offset] -ne 84 -or $Bytes[$offset + 1] -ne 75 -or
            $Bytes[$offset + 2] -ne 70 -or $Bytes[$offset + 3] -ne 66) {
            continue
        }
        if ($Bytes[$offset + 4] -ne 1 -or $Bytes[$offset + 5] -ne 128 -or
            $Bytes[$offset + 6] -ne 64 -or $Bytes[$offset + 7] -ne 1 -or
            [System.BitConverter]::ToUInt32($Bytes, $offset + 8) -ne 1024) {
            continue
        }
        if ($offset + 1044 -gt $Bytes.Length) { continue }

        [byte[]]$frame = [byte[]]::new(1024)
        [System.Array]::Copy($Bytes, $offset + 20, $frame, 0, 1024)
        $expected = [System.BitConverter]::ToUInt32($Bytes, $offset + 12)
        if ((Get-TraktCrc32 $frame) -ne $expected) { continue }

        return [pscustomobject]@{
            Frame = $frame
            Tick = [System.BitConverter]::ToUInt32($Bytes, $offset + 16)
        }
    }
    return $null
}

function Get-TraktPixel {
    param([byte[]]$Frame, [int]$X, [int]$Y)

    $index = [int][math]::Floor($Y / 8) * 128 + $X
    return (($Frame[$index] -shr ($Y -band 7)) -band 1)
}

function Get-TraktBigEndian {
    param([uint32]$Value)

    return ,([byte[]]@(
        (($Value -shr 24) -band 255),
        (($Value -shr 16) -band 255),
        (($Value -shr 8) -band 255),
        ($Value -band 255)
    ))
}

function Write-TraktPngChunk {
    param([System.IO.Stream]$Stream, [string]$Name, [byte[]]$Data)

    [byte[]]$type = [System.Text.Encoding]::ASCII.GetBytes($Name)
    [byte[]]$length = Get-TraktBigEndian ([uint32]$Data.Length)
    $Stream.Write($length, 0, 4)
    $Stream.Write($type, 0, 4)
    $Stream.Write($Data, 0, $Data.Length)
    [byte[]]$inputBytes = $type + $Data
    [byte[]]$crc = Get-TraktBigEndian (Get-TraktCrc32 $inputBytes)
    $Stream.Write($crc, 0, 4)
}

function Write-TraktPng {
    param([byte[]]$Frame, [string]$Path, [int]$Scale = 4)

    if ($Frame.Length -ne 1024 -or $Scale -lt 1 -or $Scale -gt 8) {
        throw 'Expected a 1024-byte framebuffer and scale 1..8.'
    }
    $width = 128 * $Scale
    $height = 64 * $Scale
    [byte[]]$pixels = [byte[]]::new(($width + 1) * $height)

    for ($y = 0; $y -lt $height; $y++) {
        $row = $y * ($width + 1)
        $sourceY = [int][math]::Floor($y / $Scale)
        for ($x = 0; $x -lt $width; $x++) {
            $sourceX = [int][math]::Floor($x / $Scale)
            $pixels[$row + 1 + $x] = if (Get-TraktPixel $Frame $sourceX $sourceY) { 0 } else { 255 }
        }
    }

    $compressed = [System.IO.MemoryStream]::new()
    try {
        $zlib = [System.IO.Compression.ZLibStream]::new(
            $compressed, [System.IO.Compression.CompressionLevel]::Optimal, $true
        )
        try { $zlib.Write($pixels, 0, $pixels.Length) } finally { $zlib.Dispose() }
        [byte[]]$data = $compressed.ToArray()
    } finally { $compressed.Dispose() }

    [byte[]]$header = [byte[]]::new(13)
    (Get-TraktBigEndian ([uint32]$width)).CopyTo($header, 0)
    (Get-TraktBigEndian ([uint32]$height)).CopyTo($header, 4)
    $header[8] = 8
    $header[9] = 0

    $image = [System.IO.MemoryStream]::new()
    try {
        [byte[]]$signature = @(137, 80, 78, 71, 13, 10, 26, 10)
        $image.Write($signature, 0, 8)
        Write-TraktPngChunk $image 'IHDR' $header
        Write-TraktPngChunk $image 'IDAT' $data
        Write-TraktPngChunk $image 'IEND' ([byte[]]::new(0))
        [System.IO.File]::WriteAllBytes($Path, $image.ToArray())
    } finally { $image.Dispose() }
}

function Invoke-TraktShot {
    param([string[]]$Arguments)

    $port = $null
    try {
        if ($Arguments.Count -eq 0) {
            throw 'Usage: shot COMn|RAW_FILE [--out FILE.png] [--scale 1..8] [--timeout 1..60]'
        }
        $source = $Arguments[0]
        $out = ''
        $scale = 4
        $timeout = 10

        for ($index = 1; $index -lt $Arguments.Count; $index++) {
            $option = $Arguments[$index]
            $index++
            if ($index -ge $Arguments.Count) { throw "Missing value: $option" }
            switch ($option) {
                '--out' { $out = $Arguments[$index] }
                '--scale' { $scale = [int]$Arguments[$index] }
                '--timeout' { $timeout = [int]$Arguments[$index] }
                default { throw "Unknown screenshot option: $option" }
            }
        }
        if ($scale -lt 1 -or $scale -gt 8 -or $timeout -lt 1 -or $timeout -gt 60) {
            throw 'Scale requires 1..8; timeout requires 1..60 seconds.'
        }

        $tick = $null
        if (Test-Path -LiteralPath $source -PathType Leaf) {
            [byte[]]$frame = [System.IO.File]::ReadAllBytes(
                (Resolve-Path -LiteralPath $source).Path
            )
            if ($frame.Length -ne 1024) { throw 'Raw framebuffer files contain exactly 1024 bytes.' }
        } elseif ($source -match '^COM\d+$') {
            if (-not ([System.Management.Automation.PSTypeName]'System.IO.Ports.SerialPort').Type) {
                Add-Type -AssemblyName System.IO.Ports
            }
            $port = [System.IO.Ports.SerialPort]::new(
                $source, 38400,
                [System.IO.Ports.Parity]::None, 8,
                [System.IO.Ports.StopBits]::One
            )
            $port.Handshake = [System.IO.Ports.Handshake]::None
            $port.DtrEnable = $false
            $port.RtsEnable = $false
            $port.ReadTimeout = 100
            $port.WriteTimeout = 500
            $port.Open()
            $port.DiscardInBuffer()

            $received = [System.Collections.Generic.List[byte]]::new()
            [byte[]]$buffer = [byte[]]::new(2048)
            $watch = [System.Diagnostics.Stopwatch]::StartNew()
            $retry = -1000L
            $capture = $null

            while ($watch.Elapsed.TotalSeconds -lt $timeout) {
                if ($watch.ElapsedMilliseconds - $retry -ge 1000) {
                    $port.Write("TKSS`n")
                    $retry = $watch.ElapsedMilliseconds
                }

                $available = $port.BytesToRead
                if ($available -gt 0) {
                    $count = $port.Read($buffer, 0, [math]::Min($available, $buffer.Length))
                    for ($at = 0; $at -lt $count; $at++) { $received.Add($buffer[$at]) }
                    $capture = Find-TraktFrame $received.ToArray()
                    if ($null -ne $capture) { break }
                    if ($received.Count -gt 8192) {
                        $received.RemoveRange(0, $received.Count - 2048)
                    }
                } else { Start-Sleep -Milliseconds 20 }
            }
            if ($null -eq $capture) {
                $count = [math]::Min(32, $received.Count)
                $prefix = if ($count -gt 0) {
                    [System.Convert]::ToHexString($received.GetRange(0, $count).ToArray())
                } else { 'none' }
                throw "No checked framebuffer. Received=$($received.Count) bytes; prefix=$prefix."
            }
            [byte[]]$frame = $capture.Frame
            $tick = $capture.Tick
        } else {
            throw 'Expected an existing raw file or a Windows COM port.'
        }

        if (-not $out) {
            $directory = Join-Path $global:TraceDir 'screens'
            $out = Join-Path $directory ((Get-Date -Format 'yyyyMMdd_HHmmss_fff') + '.png')
        }
        $out = [System.IO.Path]::GetFullPath($out)
        if ([System.IO.Path]::GetExtension($out) -ine '.png') {
            throw 'Screenshot output must use the .png extension.'
        }
        $null = New-Item -ItemType Directory -Path ([System.IO.Path]::GetDirectoryName($out)) -Force
        $rawPath = [System.IO.Path]::ChangeExtension($out, '.fb')
        [System.IO.File]::WriteAllBytes($rawPath, $frame)
        Write-TraktPng -Frame $frame -Path $out -Scale $scale

        return (New-TraktResult -Output @(
            "PNG: $out",
            "Raw: $rawPath",
            "CRC32: $('{0:X8}' -f (Get-TraktCrc32 $frame))",
            "Tick: $(if ($null -eq $tick) { 'not present in raw file' } else { $tick })"
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

function Invoke-TraktShotTests {
    $passed = 0
    $failures = [System.Collections.Generic.List[string]]::new()
    try {
        [byte[]]$frame = [byte[]]::new(1024)
        $frame[0] = 1
        $frame[1023] = 128
        if ((Get-TraktPixel $frame 0 0) -ne 1 -or
            (Get-TraktPixel $frame 0 1) -ne 0 -or
            (Get-TraktPixel $frame 127 63) -ne 1) {
            throw 'Framebuffer pixel order mismatch.'
        }
        $passed++

        [byte[]]$packet = [byte[]]::new(1044)
        [System.Text.Encoding]::ASCII.GetBytes('TKFB').CopyTo($packet, 0)
        $packet[4] = 1
        $packet[5] = 128
        $packet[6] = 64
        $packet[7] = 1
        [System.BitConverter]::GetBytes([uint32]1024).CopyTo($packet, 8)
        [System.BitConverter]::GetBytes([uint32](Get-TraktCrc32 $frame)).CopyTo($packet, 12)
        [System.BitConverter]::GetBytes([uint32]123).CopyTo($packet, 16)
        $frame.CopyTo($packet, 20)
        $found = Find-TraktFrame $packet
        if ($null -eq $found -or $found.Tick -ne 123 -or $found.Frame[1023] -ne 128) {
            throw 'Valid screenshot packet rejected.'
        }
        $passed++

        $packet[20] = $packet[20] -bxor 1
        if ($null -ne (Find-TraktFrame $packet)) { throw 'Corrupted screenshot accepted.' }
        $passed++
        if ($null -ne (Find-TraktFrame ([byte[]]::new(50)))) {
            throw 'Incomplete screenshot accepted.'
        }
        $passed++

        [byte[]]$encoded = Get-TraktBigEndian 0x12345678
        if ($encoded[0] -ne 0x12 -or $encoded[3] -ne 0x78) {
            throw 'PNG integer byte order mismatch.'
        }
        $passed++
    } catch { $failures.Add("FAIL: screenshot: $($_.Exception.Message)") }

    return (New-TraktResult -ExitCode ([int]($failures.Count -gt 0)) `
        -Passed $passed -Failed $failures.Count -Output $failures.ToArray())
}