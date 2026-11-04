param([string[]]$Only = @(), [double]$MaxHours = 10)
# Round 28: the fortress stand (step 2), then the plaza stand with the lake arrival after it (steps 3 and 4), one game
# run at a time. Each waits for 5+ min with no input (wait_then.ps1) and is tried again (up to 3 times) when the user
# came back during it.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\a9b51fd2-8307-4505-9047-44a5b14953cc\scratchpad\r28"
$steps = [ordered]@{
    "s2_fortress"    = "-Mode stops -Spec 'fortress:60' -SampleStats 'render.,offframe.' -TraceAt 20 -TraceFrames 60 -OutName s2_fortress -ExtraEnv @{ENGINE_PROFILE='1'}"
    "s34_plaza_lake" = "-Mode stops -Spec 'plaza:75,arrive-lake:15' -SampleStats 'render.,offframe.' -DumpEvery 5 -TraceAt 40 -TraceFrames 120 -ArriveTraceFrames 20 -ArriveStatsFrames 20 -ArriveStatsPrefix 'render.gi' -OutName s34_plaza_lake -ExtraEnv @{ENGINE_PROFILE='1'}"
}
$t0 = Get-Date
foreach ($name in $steps.Keys) {
    if ($Only.Count -and $Only -notcontains $name) { continue }
    for ($try = 1; $try -le 3; $try++) {
        $left = $MaxHours - ((Get-Date) - $t0).TotalHours
        if ($left -le 0) { "${name}: out of time"; exit 3 }
        "{0:HH:mm:ss} $name try $try" -f (Get-Date)
        & "$sp\wait_then.ps1" -MaxHours $left -Args28 $steps[$name] | Select-Object -Last 3
        $tl = "$sp\$name\timeline.txt"
        $stopped = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "STOPPED: user came back|not starting" -Quiet)
        $done = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "  done$" -Quiet)
        "{0:HH:mm:ss} $name try ${try}: done $done, stopped $stopped" -f (Get-Date)
        if (-not (Test-Path $tl)) { "${name}: no timeline - the run did not start; stopping the chain"; exit 4 }
        if ($done -and -not $stopped) { break }
        if ($try -lt 3) { Move-Item "$sp\$name" "$sp\${name}_stopped$try" -Force -ErrorAction SilentlyContinue }
    }
}
"{0:HH:mm:ss} chain finished" -f (Get-Date)