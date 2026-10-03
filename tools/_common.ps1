# Shared display and source-count helpers.
function Format-CargoError {
    param([string[]]$Lines)
    $errors = @()
    $current = $null

    foreach ($line in $Lines) {
        if ($line -match "^error(\[E\d+\])?:") {
            if ($current) { $errors += $current }
            $current = [PSCustomObject]@{
                Header = $line
                Context = @()
                Location = ""
            }
        } elseif ($current -and $line -match "^\s+-->") {
            $current.Location = $line.Trim()
        } elseif ($current -and $line -match "^\s+\|") {
            $current.Context += $line
        } elseif ($current -and $line -match "^$") {
            $errors += $current
            $current = $null
        }
    }
    if ($current) { $errors += $current }
    return $errors
}

function Format-Duration {
    param([double]$Ms)
    if ($Ms -lt 1000) { return "$([math]::Round($Ms))ms" }
    if ($Ms -lt 60000) { return "$([math]::Round($Ms / 1000, 1))s" }
    $m = [math]::Floor($Ms / 60000)
    $s = [math]::Round(($Ms % 60000) / 1000, 1)
    return "${m}m ${s}s"
}

function Write-CmdHeader {
    param([string]$Name, [string]$Desc)
    Write-Host "  $Name" -ForegroundColor Cyan -NoNewline
    Write-Host " $Desc" -ForegroundColor DarkGray
    Write-Host "  $("-" * 60)" -ForegroundColor DarkGray
}

function Write-SubStep {
    param([string]$Label, [string]$Status, [string]$Detail = "")
    $color = switch ($Status) {
        "OK"   { "Green" }
        "FAIL" { "Red" }
        "SKIP" { "Yellow" }
        "WARN" { "Yellow" }
        "RUN"  { "DarkGray" }
        default { "Gray" }
    }
    Write-Host -NoNewline "    $Label ... " -ForegroundColor White
    Write-Host -NoNewline $Status -ForegroundColor $color
    if ($Detail) { Write-Host " $Detail" -ForegroundColor DarkGray }
    else { Write-Host "" }
}

function Get-RsFileStats {
    $files = @(Get-ChildItem -LiteralPath (Join-Path $global:TraktRoot 'src') -Recurse -File -Filter '*.rs')
    $lines = 0
    $bytes = 0
    foreach ($file in $files) {
        $text = [System.IO.File]::ReadAllText($file.FullName)
        $bytes += $file.Length
        if ($text.Length -gt 0) {
            $lines += ($text -split "`n").Count
            if ($text.EndsWith("`n")) { $lines-- }
        }
    }
    return [pscustomobject]@{ Files = $files.Count; Lines = $lines; Bytes = $bytes }
}

