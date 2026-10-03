#Requires -Version 7.3
# Built-in workspace themes; no external theme files are required.

function Get-MuxThemeTable {
    return @{
        dark = @{
            Bg = 0x18181C; Panel = 0x202128; Border = 0x464854; BorderFocus = 0x5AAAFF
            Text = 0xD2D4DC; TextDim = 0x828591; TextHead = 0xF0F2F8; Accent = 0x78C88C
            StatusBg = 0x2D303C; StatusFg = 0xC8CBD6; StatusHi = 0xFADC78
        }
        light = @{
            Bg = 0xE5E7EB; Panel = 0xF3F4F6; Border = 0x8B909A; BorderFocus = 0x1769AA
            Text = 0x20242A; TextDim = 0x626975; TextHead = 0x111418; Accent = 0x237A46
            StatusBg = 0xCDD2DA; StatusFg = 0x20242A; StatusHi = 0x754F00
        }
    }
}

function Set-TraktMuxTheme {
    param([string]$Name, [switch]$Persist)

    $themes = Get-MuxThemeTable
    if (-not $themes.ContainsKey($Name)) { throw "Unknown theme: $Name" }
    $themeType = [type]'TraktTheme'
    foreach ($key in $themes[$Name].Keys) {
        $property = $themeType.GetProperty($key)
        if ($null -eq $property) { throw "Missing theme property: $key" }
        $property.SetValue($null, [int]$themes[$Name][$key])
    }
    if ($Persist) {
        $path = Join-Path $global:TraceDir 'theme.txt'
        [System.IO.File]::WriteAllText($path, $Name, [System.Text.Encoding]::ASCII)
    }
}

function Restore-MuxTheme {
    $path = Join-Path $global:TraceDir 'theme.txt'
    $name = if (Test-Path -LiteralPath $path) {
        [System.IO.File]::ReadAllText($path).Trim()
    } else { 'dark' }
    if (-not (Get-MuxThemeTable).ContainsKey($name)) { $name = 'dark' }
    Set-TraktMuxTheme -Name $name
}