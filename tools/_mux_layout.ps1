# Multiplexer layout persistence.
#Requires -Version 7.3

function ConvertTo-MuxLayoutNode {
    
    param($Node)
    if ($Node['Kind'] -eq 'pane') {
        return @{ Kind = 'pane'; Type = $Node['Pane'].GetType().Name }
    }
    return @{
        Kind = 'split'; Dir = $Node['Dir']; Ratio = [double]$Node['Ratio']
        A = (ConvertTo-MuxLayoutNode $Node['A'])
        B = (ConvertTo-MuxLayoutNode $Node['B'])
    }
}

function Export-MuxLayout {
    
    param($Root, [string]$Path)
    try {
        $dir = Split-Path $Path
        if ($dir -and -not (Test-Path $dir)) {
            New-Item -ItemType Directory -Path $dir -Force | Out-Null
        }
        $tree = ConvertTo-MuxLayoutNode $Root
        $tree | ConvertTo-Json -Depth 30 | Set-Content -Path $Path -Encoding UTF8
    } catch { }
}

function ConvertFrom-MuxLayoutNode {
    
    param($Json, $Registry, [ref]$Counter)
    if ($Json.Kind -eq 'pane') {
        foreach ($name in $Registry.Keys) {
            $entry = $Registry[$name]
            if ($entry['Type'] -eq $Json.Type) {
                $pane = & ($entry['Make'])
                if ($null -eq $pane) { return $null }
                $Counter.Value++
                return @{ Kind = 'pane'; Id = "r$($Counter.Value)"; Pane = $pane }
            }
        }
        return $null
    }
    $a = ConvertFrom-MuxLayoutNode -Json $Json.A -Registry $Registry -Counter $Counter
    if ($null -eq $a) { return $null }
    $b = ConvertFrom-MuxLayoutNode -Json $Json.B -Registry $Registry -Counter $Counter
    if ($null -eq $b) { return $null }
    $ratio = [double]$Json.Ratio
    if ($ratio -lt 0.1) { $ratio = 0.1 }
    if ($ratio -gt 0.9) { $ratio = 0.9 }
    return @{ Kind = 'split'; Dir = [string]$Json.Dir; Ratio = $ratio; A = $a; B = $b }
}

function Import-MuxLayout {
    
    param([string]$Path, $Registry)
    if (-not (Test-Path $Path)) { return $null }
    try {
        $json = Get-Content $Path -Raw | ConvertFrom-Json
        $counter = 0
        return ConvertFrom-MuxLayoutNode -Json $json -Registry $Registry -Counter ([ref]$counter)
    } catch {
        return $null
    }
}
