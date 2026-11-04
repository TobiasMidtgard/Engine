param([string[]]$Only = @(), [double]$MaxHours = 10)
# Round 24 steps a and b, one game run at a time. Each waits for 5+ min with no input (wait_then.ps1) and is
# tried again (up to 3 times) when the user came back during it.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\45d985d3-0d66-4e84-94ac-e37b77e235f5\scratchpad\r24"
$steps = [ordered]@{
    "a1_plaza_plain" = "-Mode stops -Spec 'plaza:75' -OutName a1_plaza_plain -SampleStats 'render.,offframe.' -DumpEvery 5 -ExtraEnv @{ENGINE_PROFILE='1'}"
    "a2_plaza_sets"  = "-Mode stops -Spec 'plaza:75' -OutName a2_plaza_sets -SampleStats 'render.,offframe.' -DumpEvery 5 -ExtraEnv @{ENGINE_PROFILE='1'; ENGINE_FIXED_SYNC='sets'}"
    "b_loops"        = "-Mode loops -OutName b_loops -Spec 'overview,plaza,market,forest,lake,fortress,field,orbit:1' -ExtraEnv @{PHYSICS_REPORT='240'; RUST_LOG='info'}"
}
$t0 = Get-Date
foreach ($name in $steps.Keys) {
    if ($Only.Count -and $Only -notcontains $name) { continue }
    for ($try = 1; $try -le 3; $try++) {
        $left = $MaxHours - ((Get-Date) - $t0).TotalHours
        if ($left -le 0) { "${name}: out of time"; exit 3 }
        "{0:HH:mm:ss} $name try $try" -f (Get-Date)
        & "$sp\wait_then.ps1" -MaxHours $left -Args24 $steps[$name] | Select-Object -Last 3
        $tl = "$sp\$name\timeline.txt"
        $stopped = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "STOPPED: user came back|not starting" -Quiet)
        $done = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "  done$" -Quiet)
        "{0:HH:mm:ss} $name try ${try}: done $done, stopped $stopped" -f (Get-Date)
        if ($done -and -not $stopped) { break }
        if ($try -lt 3) { Move-Item "$sp\$name" "$sp\${name}_stopped$try" -Force -ErrorAction SilentlyContinue }
    }
}
"{0:HH:mm:ss} chain finished" -f (Get-Date)
