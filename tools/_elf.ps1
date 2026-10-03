#Requires -Version 7.3
# ELF32 ARM reader for the project's fixed memory layout.

function Get-TraktU16 {
    param([byte[]]$Bytes, [long]$Offset)

    Assert-TraktSpan $Offset 2 0 $Bytes.Length 'u16'
    return [long][System.BitConverter]::ToUInt16($Bytes, [int]$Offset)
}

function Get-TraktU32 {
    param([byte[]]$Bytes, [long]$Offset)

    Assert-TraktSpan $Offset 4 0 $Bytes.Length 'u32'
    return [long][System.BitConverter]::ToUInt32($Bytes, [int]$Offset)
}

function Get-TraktElfString {
    param([byte[]]$Bytes, [object]$Table, [long]$Index)

    if ($Index -lt 0 -or $Index -ge $Table.Size) {
        throw 'String index lies outside its table.'
    }
    $start = $Table.Offset + $Index
    $end = $start
    $limit = $Table.Offset + $Table.Size
    while ($end -lt $limit -and $Bytes[$end] -ne 0) {
        if ($Bytes[$end] -gt 127) { throw 'Non-ASCII ELF name.' }
        $end++
    }
    if ($end -eq $limit) { throw 'Unterminated ELF name.' }
    return [System.Text.Encoding]::ASCII.GetString(
        $Bytes, [int]$start, [int]($end - $start)
    )
}

function Assert-TraktDisjoint {
    param([object[]]$Ranges, [string]$Name)

    $ordered = @($Ranges | Sort-Object Start)
    $end = -1L
    foreach ($range in $ordered) {
        if ($range.Start -lt $end) { throw "Overlapping ranges: $Name" }
        $end = $range.Start + $range.Size
    }
}

function Read-TraktElf {
    param([byte[]]$Bytes)

    $layout = Get-TraktLayout
    if ($Bytes.Length -lt 52 -or $Bytes.Length -gt 16 * 1024 * 1024) {
        throw 'ELF size is outside the supported range.'
    }
    if ($Bytes[0] -ne 127 -or $Bytes[1] -ne 69 -or
        $Bytes[2] -ne 76 -or $Bytes[3] -ne 70) {
        throw 'Invalid ELF magic.'
    }
    if ($Bytes[4] -ne 1 -or $Bytes[5] -ne 1 -or $Bytes[6] -ne 1) {
        throw 'Only ELF32 little-endian version 1 is supported.'
    }
    if ((Get-TraktU16 $Bytes 16) -ne 2 -or
        (Get-TraktU16 $Bytes 18) -ne 40 -or
        (Get-TraktU32 $Bytes 20) -ne 1) {
        throw 'Expected an ARM executable.'
    }
    if ((Get-TraktU16 $Bytes 40) -ne 52) {
        throw 'Unexpected ELF header size.'
    }
    $elfFlags = Get-TraktU32 $Bytes 36
    if (($elfFlags -shr 24) -ne 5) { throw 'Expected ARM EABI version 5.' }

    $entry = Get-TraktU32 $Bytes 24
    $programOffset = Get-TraktU32 $Bytes 28
    $sectionOffset = Get-TraktU32 $Bytes 32
    $programSize = Get-TraktU16 $Bytes 42
    $programCount = Get-TraktU16 $Bytes 44
    $sectionSize = Get-TraktU16 $Bytes 46
    $sectionCount = Get-TraktU16 $Bytes 48
    $nameTableIndex = Get-TraktU16 $Bytes 50

    if ($programSize -ne 32 -or $programCount -eq 0 -or $programCount -eq 65535) {
        throw 'Unsupported program header table.'
    }
    if ($programOffset -lt 52) { throw 'Program headers overlap the ELF header.' }
    Assert-TraktSpan $programOffset ($programCount * 32) 0 $Bytes.Length 'program headers'

    $loads = [System.Collections.Generic.List[object]]::new()
    $flashRanges = [System.Collections.Generic.List[object]]::new()
    $runRanges = [System.Collections.Generic.List[object]]::new()
    $ramBytes = 0L
    $flashBytes = 0L
    $ramEnd = $layout.RamOrigin
    $flashEnd = $layout.FlashOrigin
    $stackLimit = $layout.StackTop - $layout.StackSize

    for ($i = 0; $i -lt $programCount; $i++) {
        $at = $programOffset + $i * 32
        $type = Get-TraktU32 $Bytes $at
        if ($type -in 2, 3, 7) {
            throw 'Dynamic loading, interpreters, and TLS are unsupported.'
        }
        if ($type -ne 1) { continue }

        $segment = [pscustomobject]@{
            Offset = Get-TraktU32 $Bytes ($at + 4)
            Address = Get-TraktU32 $Bytes ($at + 8)
            LoadAddress = Get-TraktU32 $Bytes ($at + 12)
            FileSize = Get-TraktU32 $Bytes ($at + 16)
            MemorySize = Get-TraktU32 $Bytes ($at + 20)
            Flags = Get-TraktU32 $Bytes ($at + 24)
            Alignment = Get-TraktU32 $Bytes ($at + 28)
            Region = ''
        }
        if ($segment.FileSize -gt $segment.MemorySize) {
            throw 'A load segment has more file bytes than memory bytes.'
        }
        Assert-TraktSpan $segment.Offset $segment.FileSize 0 $Bytes.Length 'segment file data'

        $alignment = $segment.Alignment
        if ($alignment -gt 1) {
            if (($alignment -band ($alignment - 1)) -ne 0 -or
                $segment.Offset % $alignment -ne $segment.Address % $alignment) {
                throw 'Invalid load segment alignment.'
            }
        }
        if ($segment.Flags -gt 7) { throw 'Unsupported load segment flags.' }
        if ($segment.MemorySize -eq 0) { continue }

        $inFlash = $segment.Address -ge $layout.FlashOrigin -and
            $segment.Address -lt $layout.FlashOrigin + $layout.FlashSize
        $inRam = $segment.Address -ge $layout.RamOrigin -and
            $segment.Address -lt $layout.RamOrigin + $layout.RamSize

        if ($inFlash) {
            Assert-TraktSpan $segment.Address $segment.MemorySize `
                $layout.FlashOrigin $layout.FlashSize 'Flash run data'
            if (($segment.Flags -band 2) -ne 0 -or
                $segment.LoadAddress -ne $segment.Address -or
                $segment.MemorySize -ne $segment.FileSize) {
                throw 'Invalid Flash run segment.'
            }
            $segment.Region = 'Flash'
        } elseif ($inRam) {
            if (($segment.Flags -band 2) -eq 0 -or ($segment.Flags -band 1) -ne 0) {
                throw 'Static RAM must be writable and non-executable.'
            }

            if ($segment.Address -eq $layout.RetainedOrigin) {
                if ($segment.MemorySize -ne $layout.RetainedSize -or
                    $segment.FileSize -ne 0) {
                    throw 'Retained RAM must be an exact uninitialized reservation.'
                }
                Assert-TraktSpan $segment.Address $segment.MemorySize `
                    $layout.RamOrigin $layout.RamSize 'retained RAM'
                $segment.Region = 'Retained'
            } else {
                Assert-TraktSpan $segment.Address $segment.MemorySize `
                    $layout.RamOrigin ($stackLimit - $layout.RamOrigin) 'static RAM'
                $segment.Region = 'RAM'
                $ramEnd = [math]::Max($ramEnd, $segment.Address + $segment.MemorySize)
            }

            $ramBytes += $segment.MemorySize
        } else {
            throw 'A load segment runs outside project memory.'
        }

        $runRanges.Add(@{ Start = $segment.Address; Size = $segment.MemorySize })
        if ($segment.FileSize -gt 0) {
            Assert-TraktSpan $segment.LoadAddress $segment.FileSize `
                $layout.FlashOrigin $layout.FlashSize 'Flash load data'
            $flashRanges.Add(@{ Start = $segment.LoadAddress; Size = $segment.FileSize })
            $flashBytes += $segment.FileSize
            $flashEnd = [math]::Max($flashEnd, $segment.LoadAddress + $segment.FileSize)
        }
        $loads.Add($segment)
    }

    if ($loads.Count -eq 0 -or $flashRanges.Count -eq 0) {
        throw 'No loadable image exists.'
    }
    Assert-TraktDisjoint -Ranges $runRanges.ToArray() -Name 'run memory'
    Assert-TraktDisjoint -Ranges $flashRanges.ToArray() -Name 'Flash load memory'
    $first = @($flashRanges | Sort-Object Start)[0]
    if ($first.Start -ne $layout.FlashOrigin) {
        throw 'The image does not begin at the application address.'
    }

    $sections = [System.Collections.Generic.List[object]]::new()
    $symbols = [System.Collections.Generic.List[object]]::new()

    if ($sectionCount -eq 0) {
        if ($sectionOffset -ne 0 -or $nameTableIndex -ne 0) {
            throw 'Extended or inconsistent section numbering is unsupported.'
        }
    } else {
        if ($sectionSize -ne 40 -or $sectionOffset -lt 52 -or
            $nameTableIndex -eq 0 -or $nameTableIndex -ge $sectionCount) {
            throw 'Unsupported section header table.'
        }
        Assert-TraktSpan $sectionOffset ($sectionCount * 40) 0 $Bytes.Length 'section headers'

        for ($i = 0; $i -lt $sectionCount; $i++) {
            $at = $sectionOffset + $i * 40
            $section = [pscustomobject]@{
                Index = $i
                NameIndex = Get-TraktU32 $Bytes $at
                Name = ''
                Type = Get-TraktU32 $Bytes ($at + 4)
                Flags = Get-TraktU32 $Bytes ($at + 8)
                Address = Get-TraktU32 $Bytes ($at + 12)
                Offset = Get-TraktU32 $Bytes ($at + 16)
                Size = Get-TraktU32 $Bytes ($at + 20)
                Link = Get-TraktU32 $Bytes ($at + 24)
                EntrySize = Get-TraktU32 $Bytes ($at + 36)
            }
            if ($section.Type -in 6, 11 -or ($section.Flags -band 1024) -ne 0) {
                throw 'Dynamic sections and TLS are unsupported.'
            }
            if ($section.Type -ne 8) {
                Assert-TraktSpan $section.Offset $section.Size 0 $Bytes.Length 'section file data'
            }
            $sections.Add($section)
        }

        $names = $sections[$nameTableIndex]
        if ($names.Type -ne 3) { throw 'Section names do not use a string table.' }

        foreach ($section in $sections) {
            $section.Name = Get-TraktElfString $Bytes $names $section.NameIndex
            if (($section.Flags -band 2) -eq 0 -or $section.Size -eq 0) { continue }

            $owners = @($loads | Where-Object {
                $section.Address -ge $_.Address -and
                $section.Address + $section.Size -le $_.Address + $_.MemorySize
            })
            if ($owners.Count -ne 1) { throw "Unmapped allocated section: $($section.Name)" }
            $owner = $owners[0]
            if ($section.Type -ne 8) {
                $delta = $section.Address - $owner.Address
                if ($delta + $section.Size -gt $owner.FileSize -or
                    $section.Offset -ne $owner.Offset + $delta) {
                    throw "Invalid allocated section mapping: $($section.Name)"
                }
            }
        }

        foreach ($table in $sections | Where-Object { $_.Type -eq 2 }) {
            if ($table.EntrySize -ne 16 -or $table.Size % 16 -ne 0 -or
                $table.Link -ge $sections.Count -or $table.Size / 16 -gt 65536) {
                throw 'Invalid symbol table.'
            }
            $strings = $sections[$table.Link]
            if ($strings.Type -ne 3) { throw 'Symbols do not use a string table.' }

            for ($i = 0; $i -lt $table.Size / 16; $i++) {
                $at = $table.Offset + $i * 16
                $nameIndex = Get-TraktU32 $Bytes $at
                $value = Get-TraktU32 $Bytes ($at + 4)
                $size = Get-TraktU32 $Bytes ($at + 8)
                $symbolType = $Bytes[$at + 12] -band 15
                $sectionIndex = Get-TraktU16 $Bytes ($at + 14)
                $name = Get-TraktElfString $Bytes $strings $nameIndex
                if ($sectionIndex -gt 0 -and $sectionIndex -lt 65280 -and
                    $sectionIndex -ge $sections.Count) {
                    throw 'Symbol references a missing section.'
                }
                if ($name -eq '__stack_size' -and $value -ne $layout.StackSize) {
                    throw 'Linked stack budget differs from the project layout.'
                }
                if ($name -eq '__stack_limit' -and $value -ne $stackLimit) {
                    throw 'Linked stack limit differs from the project layout.'
                }
                if ($symbolType -notin 1, 2 -or $size -eq 0 -or
                    $sectionIndex -eq 0 -or $sectionIndex -ge 65280) {
                    continue
                }
                $symbols.Add([pscustomobject]@{
                    Name = $name
                    Address = $value
                    Size = $size
                    Kind = if ($symbolType -eq 2) { 'function' } else { 'object' }
                    Section = $sections[$sectionIndex].Name
                })
            }
        }
    }

    $imageSize = $flashEnd - $layout.FlashOrigin
    if ($imageSize -lt 8) { throw 'The vector prefix is missing.' }
    [byte[]]$image = [byte[]]::new([int]$imageSize)
    for ($i = 0; $i -lt $image.Length; $i++) { $image[$i] = 255 }

    foreach ($segment in $loads) {
        if ($segment.FileSize -eq 0) { continue }
        [System.Array]::Copy(
            $Bytes, [int]$segment.Offset,
            $image, [int]($segment.LoadAddress - $layout.FlashOrigin),
            [int]$segment.FileSize
        )
    }

    $initialStack = Get-TraktU32 $image 0
    $reset = Get-TraktU32 $image 4
    if ($initialStack -ne $layout.StackTop) {
        throw 'Unexpected initial stack pointer.'
    }
    if (($reset -band 1) -eq 0 -or $reset -ne $entry) {
        throw 'Reset vector and Thumb entry point disagree.'
    }
    $instructionAddress = ($entry -shr 1) -shl 1
    $entryOwners = @($loads | Where-Object {
        ($_.Flags -band 1) -ne 0 -and
        $instructionAddress -ge $_.Address -and
        $instructionAddress + 2 -le $_.Address + $_.FileSize
    })
    if ($entryOwners.Count -ne 1) { throw 'Entry point is not backed by executable file data.' }

    return [pscustomobject]@{
        Image = $image
        Entry = $entry
        Segments = $loads.ToArray()
        Sections = $sections.ToArray()
        Symbols = @($symbols | Sort-Object Size -Descending)
        Memory = [pscustomobject]@{
            ElfBytes = $Bytes.Length
            BinBytes = $image.Length
            FlashLoadBytes = $flashBytes
            FlashGapBytes = $image.Length - $flashBytes
            FlashRemainingBytes = $layout.FlashSize - $image.Length
            StaticRamBytes = $ramBytes
            StackBudgetBytes = $layout.StackSize
            RamHeadroomBytes = $stackLimit - $ramEnd
        }
    }
}