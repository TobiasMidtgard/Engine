param([string[]]$Only = @(), [double]$MaxHours = 10)
# Round 25 steps a, b and c, one game run at a time. Each waits for 5+ min with no input (wait_then.ps1) and is
# tried again (up to 3 times) when the user came back during it.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\caa7f13c-78a1-4b4e-9209-01cfa307faa4\scratchpad\r25"
$steps = [ordered]@{
    "a_plaza_plain" = "-Mode stops -Spec 'plaza:75' -OutName a_plaza_plain -SampleStats 'render.,offframe.' -DumpEvery 5 -ExtraEnv @{ENGINE_PROFILE='1'}"
    "b_loops"       = "-Mode loops -OutName b_loops -Spec 'overview,plaza,market,forest,lake,fortress,field,orbit:1' -SampleStats 'water.,render.extra_views' -FastPrefix 'render.extra_views' -FastGoAfter forest -FastHaltAfter 4 -ExtraEnv @{PHYSICS_REPORT='240'; RUST_LOG='info'}"
    "c_night"       = "-Mode night -OutName c_night -Spec 'lake,sunset' -Hours '2.5,3.5' -NightSettle 12"
}
$t0 = Get-Date
foreach ($name in $steps.Keys) {
    if ($Only.Count -and $Only -notcontains $name) { continue }
    for ($try = 1; $try -le 3; $try++) {
        $left = $MaxHours - ((Get-Date) - $t0).TotalHours
        if ($left -le 0) { "${name}: out of time"; exit 3 }
        "{0:HH:mm:ss} $name try $try" -f (Get-Date)
        & "$sp\wait_then.ps1" -MaxHours $left -Args25 $steps[$name] | Select-Object -Last 3
        $tl = "$sp\$name\timeline.txt"
        $stopped = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "STOPPED: user came back|not starting" -Quiet)
        $done = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "  done$" -Quiet)
        "{0:HH:mm:ss} $name try ${try}: done $done, stopped $stopped" -f (Get-Date)
        if ($done -and -not $stopped) { break }
        if ($try -lt 3) { Move-Item "$sp\$name" "$sp\${name}_stopped$try" -Force -ErrorAction SilentlyContinue }
    }
}
"{0:HH:mm:ss} chain finished" -f (Get-Date)
