# User aliases and startup configuration.
#Requires -Version 7.3

function Get-TraktConfigPath {
    Join-Path $global:TraktRoot '.traktrc'
}

function Import-TraktConfig {
    
    $cfg = @{ Aliases = @{}; Startup = '' }
    $p = Get-TraktConfigPath
    if (-not (Test-Path $p)) { return $cfg }
    foreach ($raw in (Get-Content $p -ErrorAction SilentlyContinue)) {
        $line = $raw.Trim()
        if (-not $line -or $line.StartsWith('#')) { continue }
        if ($line -match '^alias\s+([^\s=]+)\s*=\s*(.+)$') {
            $cfg.Aliases[$Matches[1].ToLower()] = $Matches[2].Trim()
            continue
        }
        if ($line -match '^startup\s*=\s*(.+)$') {
            $cfg.Startup = $Matches[1].Trim()
        }
    }
    return $cfg
}

function Add-TraktUserAlias {
    param([string]$Name, [string]$Expansion)
    $p = Get-TraktConfigPath
    $name = $Name.ToLower()
    $pattern = '^\s*alias\s+' + [regex]::Escape($name) + '\s*='
    $lines = if (Test-Path $p) { @(Get-Content $p) } else { @() }
    $kept = @($lines | Where-Object { $_ -notmatch $pattern })
    $kept += ("alias $name = $Expansion")
    try {
        $dir = Split-Path $p
        if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
        Set-Content -Path $p -Value $kept -Encoding UTF8
    } catch { throw }
}

function Remove-TraktUserAlias {
    param([string]$Name)
    $p = Get-TraktConfigPath
    if (-not (Test-Path $p)) { return }
    $name = $Name.ToLower()
    $pattern = '^\s*alias\s+' + [regex]::Escape($name) + '\s*='
    $kept = @(Get-Content $p | Where-Object { $_ -notmatch $pattern })
    try { Set-Content -Path $p -Value $kept -Encoding UTF8 } catch { throw }
}
