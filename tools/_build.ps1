#Requires -Version 7.3
# Cargo execution, artifact export, and build-input fingerprints.

function Get-TraktSha256 {
    param([byte[]]$Bytes)

    $hash = [System.Security.Cryptography.SHA256]::HashData($Bytes)
    return [System.Convert]::ToHexString($hash).ToLowerInvariant()
}

function Get-TraktToolchain {
    $rust = Invoke-TraktNative -Name rustc -Arguments @('-vV')
    $cargo = Invoke-TraktNative -Name cargo -Arguments @('--version')
    if ($rust.ExitCode -ne 0 -or $cargo.ExitCode -ne 0) {
        throw 'Cargo and rustc must be available.'
    }
    return [pscustomobject]@{
        Rust = $rust.Output -join "`n"
        Cargo = $cargo.Output -join "`n"
    }
}

function Get-TraktBuildFingerprint {
    param([object]$Toolchain)

    $paths = [System.Collections.Generic.List[string]]::new()
    foreach ($name in @(
        'Cargo.toml', 'Cargo.lock', 'version.json', '.cargo/config.toml',
        'tools/_layout.ps1', 'tools/_elf.ps1',
        'tools/_build.ps1', 'tools/_startup_image.ps1',
        'tools/_variants.ps1', 'tools/_version.ps1', 'tools/_build_ui.ps1'
    )) {
        $paths.Add((Join-Path $global:TraktRoot $name))
    }
    foreach ($directory in @('src', 'link')) {
        foreach ($file in Get-ChildItem -LiteralPath (Join-Path $global:TraktRoot $directory) -Recurse -File) {
            $paths.Add($file.FullName)
        }
    }

    $hash = [System.Security.Cryptography.IncrementalHash]::CreateHash(
        [System.Security.Cryptography.HashAlgorithmName]::SHA256
    )
    try {
        foreach ($path in $paths | Sort-Object) {
            $name = [System.IO.Path]::GetRelativePath($global:TraktRoot, $path).Replace('\', '/')
            $nameBytes = [System.Text.Encoding]::UTF8.GetBytes($name)
            $content = [System.IO.File]::ReadAllBytes($path)
            $hash.AppendData([System.BitConverter]::GetBytes([long]$nameBytes.Length))
            $hash.AppendData($nameBytes)
            $hash.AppendData([System.BitConverter]::GetBytes([long]$content.Length))
            $hash.AppendData($content)
        }
        $hash.AppendData([System.Text.Encoding]::UTF8.GetBytes(
            $Toolchain.Rust + "`n" + $Toolchain.Cargo
        ))
        return [System.Convert]::ToHexString($hash.GetHashAndReset()).ToLowerInvariant()
    } finally { $hash.Dispose() }
}

function Assert-TraktBuildEnvironment {
    foreach ($variable in Get-ChildItem Env:) {
        if ($variable.Name -in @(
            'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
            'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
            'CARGO_BUILD_RUSTFLAGS',
            'CARGO_TARGET_THUMBV6M_NONE_EABI_RUSTFLAGS',
            'CARGO_TARGET_THUMBV6M_NONE_EABI_LINKER'
        ) -or $variable.Name -like 'CARGO_PROFILE_*') {
            if ($variable.Value) {
                throw "External build override is not admitted: $($variable.Name)"
            }
        }
    }
}

function Invoke-TraktCargo {
    param([string[]]$Arguments)

    $command = Get-Command cargo -CommandType Application -ErrorAction Stop |
        Select-Object -First 1
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $command.Source
    $startInfo.WorkingDirectory = $global:TraktRoot
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.CreateNoWindow = $true
    foreach ($argument in $Arguments) { $startInfo.ArgumentList.Add($argument) }
    $startInfo.Environment['CARGO_TERM_COLOR'] = 'never'

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $stdout = [System.Collections.Generic.List[string]]::new()
    $stderr = [System.Collections.Generic.List[string]]::new()
    $started = $false

    try {
        $started = $process.Start()
        if (-not $started) { throw 'Cargo could not be started.' }

        Set-TraktBuildDetail "Cargo PID $($process.Id); waiting for compiler messages"

        $outTask = $process.StandardOutput.ReadLineAsync()
        $errTask = $process.StandardError.ReadLineAsync()
        $outDone = $false
        $errDone = $false

        while (-not $outDone -or -not $errDone) {
            $activity = $false

            if (-not $outDone -and $outTask.IsCompleted) {
                $line = $outTask.GetAwaiter().GetResult()
                if ($null -eq $line) {
                    $outDone = $true
                } else {
                    $stdout.Add($line)
                    Receive-TraktCargoEvent -Line $line -Stream stdout
                    $outTask = $process.StandardOutput.ReadLineAsync()
                }
                $activity = $true
            }

            if (-not $errDone -and $errTask.IsCompleted) {
                $line = $errTask.GetAwaiter().GetResult()
                if ($null -eq $line) {
                    $errDone = $true
                } else {
                    $stderr.Add($line)
                    Receive-TraktCargoEvent -Line $line -Stream stderr
                    $errTask = $process.StandardError.ReadLineAsync()
                }
                $activity = $true
            }

            Update-TraktBuildUi
            if (-not $activity) { Start-Sleep -Milliseconds 10 }
        }

        # Streams can close before the process terminates.
        while (-not $process.WaitForExit(50)) {
            Update-TraktBuildUi
        }

        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdout.ToArray()
            Stderr = $stderr.ToArray()
        }
    } finally {
        try {
            if ($started -and -not $process.HasExited) {
                $process.Kill($true)
                $process.WaitForExit()
            }
        } finally {
            $process.Dispose()
        }
    }
}

function ConvertFrom-TraktCargo {
    param([object]$Run)

    $messages = [System.Collections.Generic.List[string]]::new()
    $artifacts = [System.Collections.Generic.List[string]]::new()
    $finished = 0
    $success = $false

    foreach ($line in $Run.Stdout) {
        if (-not $line.Trim()) { continue }
        $record = ConvertFrom-Json -InputObject $line -ErrorAction Stop
        switch ($record.reason) {
            'compiler-message' {
                if ($record.message.rendered) {
                    foreach ($text in $record.message.rendered -split '\r?\n') {
                        if ($text.Length -gt 0) { $messages.Add($text) }
                    }
                }
            }
            'compiler-artifact' {
                if ($record.target.name -eq 'trakt' -and
                    $record.target.kind -contains 'bin' -and $record.executable) {
                    $artifacts.Add([string]$record.executable)
                }
            }
            'build-finished' {
                $finished++
                $success = [bool]$record.success
            }
            'build-script-executed' {
                throw 'Build scripts are prohibited.'
            }
        }
    }
    foreach ($line in $Run.Stderr) { $messages.Add($line) }

    if ($Run.ExitCode -eq 0 -and ($finished -ne 1 -or -not $success)) {
        throw 'Cargo did not report exactly one successful build completion.'
    }
    return [pscustomobject]@{
        ExitCode = $Run.ExitCode
        Output = $messages.ToArray()
        Artifacts = @($artifacts | Sort-Object -Unique)
    }
}

function Format-TraktImageReport {
    param([object]$Report, [object]$Elf, [switch]$Brief)

    $memory = $Elf.Memory

    if ($Brief) {
        return @(
            "Firmware $($Report.Firmware.Version) | $($Report.Profile)/$($Report.Variant)",
            ('Flash {0} B; remaining {1} B' -f
                $memory.FlashLoadBytes, $memory.FlashRemainingBytes),
            ('Static RAM {0} B; headroom {1} B; subsets: diagnostics {2} B, Spectrum {3} B, UART {4} B' -f
                $memory.StaticRamBytes, $memory.RamHeadroomBytes,
                $memory.DiagnosticRamBytes, $memory.SpectrumRamBytes,
                $memory.UartRamBytes),
            ('Stack reserved {0} B; runtime usage is not established by this build.' -f
                $memory.StackBudgetBytes)
        )
    }
    $lines = [System.Collections.Generic.List[string]]::new()
    $scope = if ($Report.Profile -ceq 'release' -and $Report.BenchTestAllowed) {
        'release image; hardware behavior is not verified by this command'
    } else {
        'development image; hardware behavior is not verified by this command'
    }
    $lines.Add("Trakt $($Report.Profile): $($Report.ImageKind); $scope.")
    $lines.Add("Firmware $($Report.Firmware.Version) | $($Report.Firmware.Label)")
    $modules = if ($Report.Modules -join ',') {
        $Report.Modules -join ', '
    } else { 'none' }
    $lines.Add("Variant $($Report.Variant) | optional modules: $modules")
    $lines.Add(('Entry              0x{0:X8}' -f $Elf.Entry))
    foreach ($field in @(
        @{ Name = 'ELF file'; Value = $memory.ElfBytes },
        @{ Name = 'BIN image'; Value = $memory.BinBytes },
        @{ Name = 'Flash load'; Value = $memory.FlashLoadBytes },
        @{ Name = 'Flash gaps'; Value = $memory.FlashGapBytes },
        @{ Name = 'Flash remaining'; Value = $memory.FlashRemainingBytes },
        @{ Name = 'Static RAM'; Value = $memory.StaticRamBytes },
        @{ Name = 'Diag RAM subset'; Value = $memory.DiagnosticRamBytes },
        @{ Name = 'Scope RAM subset'; Value = $memory.SpectrumRamBytes },
        @{ Name = 'UART RAM subset'; Value = $memory.UartRamBytes },
        @{ Name = 'Retained RAM'; Value = $memory.RetainedRamBytes },
        @{ Name = 'Stack budget'; Value = $memory.StackBudgetBytes },
        @{ Name = 'Main stack'; Value = $memory.MainStackBudgetBytes },
        @{ Name = 'Fault stack'; Value = $memory.FaultStackBudgetBytes },
        @{ Name = 'RAM headroom'; Value = $memory.RamHeadroomBytes }
    )) {
        $lines.Add(('{0,-18} {1,8} B' -f $field.Name, $field.Value))
    }
    $lines.Add('Stack: linked reservation only; device watermark is reported separately.')
    $allocated = @($Elf.Sections | Where-Object {
        ($_.Flags -band 2) -ne 0 -and $_.Size -gt 0
    })
    if ($allocated.Count -gt 0) {
        $lines.Add('')
        $lines.Add('Allocated sections:')
        foreach ($section in $allocated) {
            $lines.Add(('  {0,-20} 0x{1:X8} {2,8} B' -f
                $section.Name, $section.Address, $section.Size))
        }
    }
    $top = @($Elf.Symbols | Select-Object -First 12)
    if ($top.Count -gt 0) {
        $lines.Add('')
        $lines.Add('Largest sized symbols; not an exact source-module cost:')
        foreach ($symbol in $top) {
            $lines.Add(('  {0,8} B  {1}' -f $symbol.Size, $symbol.Name))
        }
    }
    return $lines.ToArray()
}

function Invoke-TraktBuild {
    param(
        [ValidateSet('dev', 'release')][string]$Profile = 'dev',
        [switch]$CheckOnly,
        [ValidateSet('full', 'radio', 'compact', 'standalone', 'debug', 'full-fam')][string]$Variant = 'full',
        [switch]$Brief
    )

    $operation = if ($CheckOnly) { 'check' } else { 'build' }
    $steps = if ($CheckOnly) { 3 } else { 5 }
    $owner = Start-TraktBuildUi -Label "$operation $Profile/$Variant" -Steps $steps
    $paths = $null
    $result = $null

    try {
        $paths = Get-TraktArtifactPaths -Profile $Profile -Variant $Variant

        $result = & {
            $variantInfo = Get-TraktVariant $Variant
            $label = "$Variant/$Profile"

            Enter-TraktBuildStep "$label inputs" 'Source audit and toolchain'
            if (-not $CheckOnly -and (Test-Path -LiteralPath $paths.Report)) {
                Remove-Item -LiteralPath $paths.Report -Force
            }

            $audit = Invoke-TraktAudit
            if ($audit.ExitCode -ne 0) { return $audit }

            Set-TraktBuildDetail 'Checking environment and target libraries'
            Assert-TraktBuildEnvironment
            $doctor = Invoke-TraktDoctor
            if ($doctor.ExitCode -ne 0) { return $doctor }

            $toolchain = Get-TraktToolchain
            Set-TraktBuildDetail 'Hashing build inputs'
            $before = Get-TraktBuildFingerprint -Toolchain $toolchain
            $modules = if ($variantInfo.Features.Count) {
                $variantInfo.Features -join ','
            } else { 'none' }
            Exit-TraktBuildStep -Detail "thumbv6m-none-eabi; modules=$modules"

            Enter-TraktBuildStep "$label Cargo $operation" 'trakt / thumbv6m-none-eabi'
            $arguments = @(
                $operation, '--offline', '--locked',
                '--manifest-path', (Join-Path $global:TraktRoot 'Cargo.toml'),
                '--target', 'thumbv6m-none-eabi',
                '--target-dir', (Join-Path $global:TraktRoot "target/variants/$Variant"),
                '--bin', 'trakt', '--profile', $Profile, '--message-format=json'
            ) + $variantInfo.CargoArguments

            $run = ConvertFrom-TraktCargo (Invoke-TraktCargo -Arguments $arguments)
            if ($run.ExitCode -ne 0) {
                Exit-TraktBuildStep -Status FAIL -Detail "Cargo exit $($run.ExitCode)"
                return (New-TraktResult -ExitCode $run.ExitCode -Output $run.Output)
            }

            $warnings = $global:TraktBuildUi.Step.Warnings
            Exit-TraktBuildStep -Detail 'Cargo output validated'
            if ($warnings -gt 0) {
                foreach ($line in $run.Output) { Write-Host $line }
            }

            Enter-TraktBuildStep "$label fingerprint" 'Verifying input stability'
            if ((Get-TraktBuildFingerprint -Toolchain $toolchain) -cne $before) {
                throw 'Build inputs changed during Cargo execution.'
            }
            Exit-TraktBuildStep -Detail 'Inputs unchanged'

            if ($CheckOnly) {
                return (New-TraktResult -Output (@($run.Output) + @(
                    'Target code checked. Linking and image validation were not performed.'
                )))
            }

            Enter-TraktBuildStep "$label ELF validation" 'Load image, memory, vectors, module records'
            if ($run.Artifacts.Count -ne 1) {
                throw 'Expected exactly one Trakt executable artifact.'
            }

            $artifact = [System.IO.Path]::GetFullPath($run.Artifacts[0])
            $targetPrefix = [System.IO.Path]::GetFullPath(
                (Join-Path $global:TraktRoot 'target')
            ).TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
            if (-not $artifact.StartsWith(
                $targetPrefix, [System.StringComparison]::OrdinalIgnoreCase
            )) {
                throw 'Cargo returned an artifact outside the project target directory.'
            }

            $bytes = [System.IO.File]::ReadAllBytes($artifact)
            $elf = Read-TraktElf -Bytes $bytes
            Assert-TraktStartupImage -Elf $elf -Variant $Variant
            Exit-TraktBuildStep -Detail (
                'Flash {0} B; static RAM {1} B; headroom {2} B' -f
                $elf.Memory.FlashLoadBytes, $elf.Memory.StaticRamBytes,
                $elf.Memory.RamHeadroomBytes
            )

            Enter-TraktBuildStep "$label export" 'ELF, BIN, hashes, and artifact report'
            $report = [pscustomobject]@{
                Schema = 2
                Variant = $Variant
                Modules = $variantInfo.Features
                Firmware = Get-TraktVersionText (Read-TraktVersion)
                Profile = $Profile
                ImageKind = (Get-TraktLayout).ImageKind
                InstallAllowed = $false
                BenchTestAllowed = ($Profile -ceq 'release')
                HardwareVerified = $false
                InputSha256 = $before
                ElfSha256 = Get-TraktSha256 $bytes
                BinSha256 = Get-TraktSha256 $elf.Image
                Toolchain = $toolchain
                Memory = $elf.Memory
                Entry = $elf.Entry
                Segments = $elf.Segments
                Sections = $elf.Sections
                LargestSymbols = @($elf.Symbols | Select-Object -First 32)
            }

            $null = New-Item -ItemType Directory -Path $paths.Directory -Force
            [System.IO.File]::WriteAllBytes($paths.Elf, $bytes)
            [System.IO.File]::WriteAllBytes($paths.Bin, $elf.Image)
            $json = ConvertTo-Json -InputObject $report -Depth 8
            [System.IO.File]::WriteAllText(
                $paths.Report, $json, [System.Text.UTF8Encoding]::new($false)
            )
            Exit-TraktBuildStep -Detail 'Artifacts exported'
            Add-TraktBuildImage -Profile $Profile -Variant $Variant `
                -Elf $elf -Directory $paths.Directory

            return (New-TraktResult -Passed 1 -Output (@($run.Output) +
                @(Format-TraktImageReport -Report $report -Elf $elf -Brief:$Brief) + @(
                    "Artifacts: $($paths.Directory)"
                )))
        }
    } catch {
        if (-not $CheckOnly -and $null -ne $paths -and
            (Test-Path -LiteralPath $paths.Report)) {
            Remove-Item -LiteralPath $paths.Report -Force
        }
        $result = New-TraktResult -ExitCode 1 -Output @($_.Exception.Message)
    } finally {
        Finish-TraktBuildUi -Owner $owner -Result $result
    }

    return $result
}

function Invoke-TraktInspect {
    param(
        [ValidateSet('dev', 'release')][string]$Profile = 'release',
        [ValidateSet('full', 'radio', 'compact', 'standalone', 'debug', 'full-fam')][string]$Variant = 'full'
    )

    try {
        $variantInfo = Get-TraktVariant $Variant
        $paths = Get-TraktArtifactPaths -Profile $Profile -Variant $Variant
        foreach ($path in @($paths.Report, $paths.Elf, $paths.Bin)) {
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
                throw "Build artifacts are missing for profile $Profile."
            }
        }
        $report = [System.IO.File]::ReadAllText($paths.Report) | ConvertFrom-Json
        if ($report.Schema -ne 2 -or $report.Profile -cne $Profile -or
            $report.Variant -cne $Variant -or
            ($report.Modules -join ',') -cne ($variantInfo.Features -join ',') -or
            $report.ImageKind -cne (Get-TraktLayout).ImageKind -or
            $report.InstallAllowed -ne $false -or
            $report.HardwareVerified -ne $false -or
            $report.BenchTestAllowed -ne ($Profile -ceq 'release')) {
            throw 'Unexpected artifact report.'
        }
        Assert-TraktBuildEnvironment
        $fingerprint = Get-TraktBuildFingerprint -Toolchain (Get-TraktToolchain)
        if ($report.InputSha256 -cne $fingerprint) {
            throw 'Artifacts are stale: sources, configuration, or toolchain changed.'
        }

        $bytes = [System.IO.File]::ReadAllBytes($paths.Elf)
        $binary = [System.IO.File]::ReadAllBytes($paths.Bin)
        if ($report.ElfSha256 -cne (Get-TraktSha256 $bytes) -or
            $report.BinSha256 -cne (Get-TraktSha256 $binary)) {
            throw 'Artifact hash mismatch.'
        }
        $elf = Read-TraktElf -Bytes $bytes
        Assert-TraktStartupImage -Elf $elf -Variant $Variant
        if ((Get-TraktSha256 $elf.Image) -cne (Get-TraktSha256 $binary)) {
            throw 'BIN does not match the ELF load data.'
        }
        return (New-TraktResult -Passed 1 -Output @(
            Format-TraktImageReport -Report $report -Elf $elf
        ))
    } catch {
        return (New-TraktResult -ExitCode 1 -Output @($_.Exception.Message))
    }
}