#Requires -Version 7.3
# One command catalog for help, completion, and dispatch.

$global:TraktCommands = [ordered]@{
    help = @{ Summary = 'Show the command reference'; Options = @() }
    doctor = @{ Summary = 'Inspect the local toolchain'; Options = @() }
    assign = @{ Summary = 'Name the next minor version without building'; Options = @() }
    channels = @{
        Summary = 'Export or stage a checked channel bank over UART'
        Options = @('export', 'import', 'validate')
    }
    shot = @{ Summary = 'Capture a framebuffer from UART or convert a raw file'; Options = @('--out', '--scale', '--timeout') }
    audit = @{ Summary = 'Check active source and dependency policy'; Options = @() }
    status = @{ Summary = 'Show Git and project state'; Options = @() }
    calib = @{ Summary = 'Inspect a saved calibration block'; Options = @('--offset') }
    build = @{
        Summary = 'Reserve a version and build the selected profile'
        Options = @('dev', 'release', '--assign', '--ignore-increment', '--variant=full', '--variant=radio', '--variant=compact', '--variant=standalone', '--variant=debug', '--variant=full-fam')
    }
    check = @{ Summary = 'Check target code without linking'; Options = @('dev', 'release', '--variant=full', '--variant=radio', '--variant=compact', '--variant=standalone', '--variant=debug', '--variant=full-fam') }
    inspect = @{ Summary = 'Verify exported ELF, BIN, and memory accounting'; Options = @('dev', 'release', '--variant=full', '--variant=radio', '--variant=compact', '--variant=standalone', '--variant=debug', '--variant=full-fam') }
    frames = @{ Summary = 'Emit assembly and show foreground task prologues'; Options = @('dev', 'release', '--variant=full', '--variant=radio', '--variant=compact', '--variant=standalone', '--variant=debug', '--variant=full-fam') }
    test = @{ Summary = 'Run host checks'; Options = @('tools', 'all') }
    ci = @{
        Summary = 'Run selected gates with one firmware version'
        Options = @('audit', 'tools', 'firmware', '--assign', '--ignore-increment', '--variant=all', '--variant=full', '--variant=radio', '--variant=compact', '--variant=standalone', '--variant=debug', '--variant=full-fam')
    }
    trace = @{ Summary = 'Inspect command traces'; Options = @('last', 'list') }
    alias = @{ Summary = 'List, save, or remove user aliases'; Options = @('remove') }
    stub = @{ Summary = 'Export a best-effort Rust API digest'; Options = @() }
    mux = @{ Summary = 'Open the terminal workspace'; Options = @('--reset') }
    theme = @{ Summary = 'Select a workspace theme'; Options = @('dark', 'light') }
    live = @{ Summary = 'Radio telemetry; currently blocked'; Options = @() }
    clean = @{ Summary = 'Remove generated artifacts with confirmation'; Options = @('target', 'traces', '--yes') }
}

$global:Aliases = @{
    b = 'build'; c = 'check'; t = 'test'; s = 'status'; h = 'help'
    q = 'exit'; tr = 'trace'; st = 'stub'; mu = 'mux'; th = 'theme'
    a = 'alias'; cl = 'clean'; cls = 'clear'; '!!' = 'repeat'
}