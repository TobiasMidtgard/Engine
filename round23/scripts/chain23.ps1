param([string[]]$Only = @(), [double]$MaxHours = 10)
# Round 23 steps 2-4, one game run at a time. Each waits for 5+ min with no input (wait_then.ps1) and is
# tried again (up to 3 times) when the user came back during it.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\22c660b7-08e6-4a7e-aff7-525fad477790\scratchpad"
$steps = [ordered]@{
    "s2_route"         = "-Mode route -OutName s2_route -FastPrefix 'render.cpu.,offframe.' -SampleStats 'offframe.'"
    "s2_route_up"      = "-Mode route -Spec up -OutName s2_route_up -FastPrefix 'render.cpu.,offframe.' -SampleStats 'offframe.' -UpKm '8,16,25,42'"
    "s3_loops"         = "-Mode loops -OutName s3_loops -Spec 'overview,plaza,market,forest,lake,fortress,field,orbit:2' -ExtraEnv @{PHYSICS_REPORT='240'; RUST_LOG='info'}"
    "s3_loops_scale07" = "-Mode loops -OutName s3_loops_scale07 -Spec 'overview,plaza,market,forest,lake,fortress,field,orbit:2' -ExtraEnv @{PHYSICS_REPORT='240'; RUST_LOG='info'; ENGINE_RENDER_SCALE='0.7'}"
    "s4_night"         = "-Mode night -OutName s4_night -Spec 'plaza,night,sunset,lake' -Hours '22.5,2.5,3.5' -NightSettle 12"
}
$t0 = Get-Date
foreach ($name in $steps.Keys) {
    if ($Only.Count -and $Only -notcontains $name) { continue }
    for ($try = 1; $try -le 3; $try++) {
        $left = $MaxHours - ((Get-Date) - $t0).TotalHours
        if ($left -le 0) { "${name}: out of time"; exit 3 }
        "{0:HH:mm:ss} $name try $try" -f (Get-Date)
        & "$sp\wait_then.ps1" -MaxHours $left -Args23 $steps[$name] | Select-Object -Last 3
        $tl = "$sp\$name\timeline.txt"
        $stopped = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "STOPPED: user came back|not starting" -Quiet)
        $done = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "  done$" -Quiet)
        "{0:HH:mm:ss} $name try ${try}: done $done, stopped $stopped" -f (Get-Date)
        if ($done -and -not $stopped) { break }
        if ($try -lt 3) { Move-Item "$sp\$name" "$sp\${name}_stopped$try" -Force -ErrorAction SilentlyContinue }
    }
}
"{0:HH:mm:ss} chain finished" -f (Get-Date)
