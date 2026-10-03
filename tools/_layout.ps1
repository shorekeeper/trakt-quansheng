#Requires -Version 7.3
# Project-specific image limits. Addresses are checked against linked data.

function Get-TraktLayout {
    return [pscustomobject]@{
        FlashOrigin = [long]0x08002800
        FlashSize = [long](118 * 1024)
        RamOrigin = [long]0x20000000
        RamSize = [long](16 * 1024)
        RetainedOrigin = [long]0x20003FC0
        RetainedSize = [long]64
        StackTop = [long]0x20003FC0
        StackSize = [long]8192
        FaultStackSize = [long]1024
        VectorBytes = [long]192
        Target = 'thumbv6m-none-eabi'
        ImageKind = 'transmit-check'
    }
}

function Assert-TraktSpan {
    param(
        [long]$Offset,
        [long]$Size,
        [long]$Origin,
        [long]$Capacity,
        [string]$Name
    )

    if ($Offset -lt $Origin -or $Size -lt 0 -or
        $Offset - $Origin -gt $Capacity -or
        $Size -gt $Capacity - ($Offset - $Origin)) {
        throw "Out-of-range span: $Name"
    }
}

function Get-TraktArtifactPaths {
    param(
        [ValidateSet('dev', 'release')][string]$Profile,
        [ValidateSet('full', 'radio', 'compact', 'standalone', 'debug', 'full-fam')][string]$Variant = 'full'
    )

    $relative = if ($Variant -ceq 'full') {
        "target/artifacts/$Profile"
    } else {
        "target/artifacts/$Variant/$Profile"
    }
    $directory = Join-Path $global:TraktRoot $relative
    return [pscustomobject]@{
        Directory = $directory
        Elf = Join-Path $directory 'trakt.elf'
        Bin = Join-Path $directory 'trakt.bin'
        Report = Join-Path $directory 'trakt.json'
    }
}