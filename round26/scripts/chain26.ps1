param([string[]]$Only = @(), [double]$MaxHours = 10)
# Round 26 steps a and b, one game run at a time. Each waits for 5+ min with no input (wait_then.ps1) and is
# tried again (up to 3 times) when the user came back during it.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\a9b51fd2-8307-4505-9047-44a5b14953cc\scratchpad\r26"
$steps = [ordered]@{
    "a_plaza_plain" = "-Mode stops -Spec 'plaza:75' -OutName a_plaza_plain -SampleStats 'render.,offframe.' -DumpEvery 5 -TraceAt 40 -TraceFrames 120 -ExtraEnv @{ENGINE_PROFILE='1'}"
    "b_loops"       = "-Mode loops -OutName b_loops -Spec 'overview,plaza,market,forest,lake,fortress,field,orbit:1' -SampleStats 'water.,render.extra_views' -FastPrefix 'render.extra_views' -FastGoAfter forest -FastHaltAfter 4 -TraceStops 'plaza,market,lake,fortress' -TraceAfter 10 -TraceStopFrames 60 -ArrivalTrace lake -ArrivalFrames 30 -ExtraEnv @{PHYSICS_REPORT='240'; RUST_LOG='info'}"
}
$t0 = Get-Date
foreach ($name in $steps.Keys) {
    if ($Only.Count -and $Only -notcontains $name) { continue }
    for ($try = 1; $try -le 3; $try++) {
        $left = $MaxHours - ((Get-Date) - $t0).TotalHours
        if ($left -le 0) { "${name}: out of time"; exit 3 }
        "{0:HH:mm:ss} $name try $try" -f (Get-Date)
        & "$sp\wait_then.ps1" -MaxHours $left -Args26 $steps[$name] | Select-Object -Last 3
        $tl = "$sp\$name\timeline.txt"
        $stopped = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "STOPPED: user came back|not starting" -Quiet)
        $done = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "  done$" -Quiet)
        "{0:HH:mm:ss} $name try ${try}: done $done, stopped $stopped" -f (Get-Date)
        if ($done -and -not $stopped) { break }
        if ($try -lt 3) { Move-Item "$sp\$name" "$sp\${name}_stopped$try" -Force -ErrorAction SilentlyContinue }
    }
}
"{0:HH:mm:ss} chain finished" -f (Get-Date)
