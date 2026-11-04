param([string[]]$Only = @(), [double]$MaxHours = 10)
# Round 27: the plaza stand three ways (a1 plain, a2 ENGINE_EXECUTOR=single, a3 single for every main schedule but
# FixedUpdate), then the fortress stand (b), one game run at a time. Each waits for 5+ min with no input
# (wait_then.ps1) and is tried again (up to 3 times) when the user came back during it.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\a9b51fd2-8307-4505-9047-44a5b14953cc\scratchpad\r27"
$plaza = "-Mode stops -Spec 'plaza:75' -SampleStats 'render.,offframe.' -DumpEvery 5 -TraceAt 40 -TraceFrames 120"
$steps = [ordered]@{
    "a1_plain"         = "$plaza -OutName a1_plain -ExtraEnv @{ENGINE_PROFILE='1'}"
    "a2_single"        = "$plaza -OutName a2_single -ExtraEnv @{ENGINE_PROFILE='1'; ENGINE_EXECUTOR='single'}"
    "a3_single_but_fixed" = "$plaza -OutName a3_single_but_fixed -ExtraEnv @{ENGINE_PROFILE='1'; ENGINE_EXECUTOR='single:Update,PostUpdate,First,Last,PreUpdate,FixedPreUpdate,FixedPostUpdate'}"
    "b_fortress"       = "-Mode stops -Spec 'fortress:60' -SampleStats 'render.,offframe.' -TraceAt 20 -TraceFrames 60 -OutName b_fortress -ExtraEnv @{ENGINE_PROFILE='1'}"
}
$t0 = Get-Date
foreach ($name in $steps.Keys) {
    if ($Only.Count -and $Only -notcontains $name) { continue }
    for ($try = 1; $try -le 3; $try++) {
        $left = $MaxHours - ((Get-Date) - $t0).TotalHours
        if ($left -le 0) { "${name}: out of time"; exit 3 }
        "{0:HH:mm:ss} $name try $try" -f (Get-Date)
        & "$sp\wait_then.ps1" -MaxHours $left -Args27 $steps[$name] | Select-Object -Last 3
        $tl = "$sp\$name\timeline.txt"
        $stopped = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "STOPPED: user came back|not starting" -Quiet)
        $done = (Test-Path $tl) -and (Select-String -Path $tl -Pattern "  done$" -Quiet)
        "{0:HH:mm:ss} $name try ${try}: done $done, stopped $stopped" -f (Get-Date)
        if ($done -and -not $stopped) { break }
        if ($try -lt 3) { Move-Item "$sp\$name" "$sp\${name}_stopped$try" -Force -ErrorAction SilentlyContinue }
    }
}
"{0:HH:mm:ss} chain finished" -f (Get-Date)
