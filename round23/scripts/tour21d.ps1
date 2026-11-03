param([string]$Run)
# Step 3 (--tour plaza --fly, ENGINE_PROFILE=1, PHYSICS_REPORT=600): every 5 s frame profile's head, schedules,
# physics_simulate row and its "ordering floor" block (with the chain: lines) as printed; physics.step_ms from the
# sampled engine.stats (per step, the last step's); fixed steps per frame from engine.status (fixed_tick / frame);
# the physics reports' summary lines, "awake:" lists and "water samples a step".
[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\22c660b7-08e6-4a7e-aff7-525fad477790\scratchpad"
$d = "$sp\$Run"; $L = [IO.File]::ReadAllLines("$d\stderr.log")
function TS($line) { if ($line -match '^(20\d\d-\d\d-\d\dT[\d:.]+)Z') { return [datetime]::Parse($Matches[1]) } }
"==== frame profiles"
for ($j = 0; $j -lt $L.Count; $j++) {
    if ($L[$j] -notmatch 'engine_core::profile: frame profile') { continue }
    $b = @(); for ($k = $j + 1; $k -lt $L.Count -and $L[$k] -notmatch '^20\d\d-'; $k++) { $b += $L[$k] }
    "-- {0:HH:mm:ss.fff} {1}" -f (TS $L[$j]), $b[0].Trim()
    "   " + $b[1].Trim()
    $sc = $false; $sched = @()
    foreach ($s in $b) { if ($s -match '^schedule\s+ms/frame') { $sc = $true; continue }; if ($sc -and $s -match '^(\w+)\s+([\d.]+)\s+([\d.]+)') { $sched += "$($Matches[1]) $($Matches[2]) x$($Matches[3])" } elseif ($sc) { $sc = $false } }
    "   schedules: " + ($sched -join ", ")
    $ps = $b | Where-Object { $_ -match '^engine_physics::physics_simulate\s' } | Select-Object -First 1
    if ($ps) { "   physics_simulate (ms/frame, max ms, runs/frame): " + (($ps -replace '^engine_physics::physics_simulate\s+', '').Trim() -replace '\s+', ' / ') }
    $al = $b | Where-Object { $_ -match '^engine_physics::physics_simulate\s+[\d.]+$' } | Select-Object -Last 1
    if ($al) { "   physics_simulate ran alone: " + ($al -replace '^engine_physics::physics_simulate\s+', '') }
    $i0 = [array]::FindIndex([string[]]$b, [Predicate[string]]{ param($x) $x -match '^ordering floor' })
    # The ordering floor is the report's last block (profile.rs appends it at the end).
    if ($i0 -ge 0) { for ($k = $i0; $k -lt $b.Count; $k++) { if ($b[$k].Trim()) { "   " + $b[$k] } } }
}
"==== physics.step_ms (sampled engine.stats, 2 Hz) and fixed steps per frame (engine.status)"
if (Test-Path "$d\sampled_stats.jsonl") {
    $st = @(Get-Content "$d\sampled_stats.jsonl" | ForEach-Object { $t, $j = $_ -split ' ', 2; $m = ($j | ConvertFrom-Json).result.metrics; if ($m.'physics.step_ms' -ne $null) { [pscustomobject]@{ t = [datetime]::Parse($t).ToUniversalTime(); step = [double]$m.'physics.step_ms'; sub = $m.'physics.substeps'; awake = $m.'physics.awake'; bodies = $m.'physics.bodies'; regions = $m.'physics.regions'; contacts = $m.'physics.contacts' } } })
    if ($st.Count) { $v = $st | ForEach-Object step | Sort-Object
        "samples {0}: physics.step_ms mean {1:N2}, median {2:N2}, p90 {3:N2}, max {4:N2}; substeps {5}; awake {6}-{7} of {8} bodies; regions {9}; contacts {10}-{11}" -f $st.Count, ($v | Measure-Object -Average).Average, $v[[int]($v.Count / 2)], $v[[int][math]::Min($v.Count - 1, [math]::Ceiling($v.Count * 0.9) - 1)], $v[-1],
            (($st | ForEach-Object sub | Sort-Object -Unique) -join "/"), ($st | ForEach-Object { [double]$_.awake } | Measure-Object -Minimum).Minimum, ($st | ForEach-Object { [double]$_.awake } | Measure-Object -Maximum).Maximum, $st[-1].bodies, $st[-1].regions, ($st | ForEach-Object { [double]$_.contacts } | Measure-Object -Minimum).Minimum, ($st | ForEach-Object { [double]$_.contacts } | Measure-Object -Maximum).Maximum
        "per 5 s (mean / max physics.step_ms):"
        $st | Group-Object { [int](($_.t - $st[0].t).TotalSeconds / 5) } | ForEach-Object { $x = $_.Group | ForEach-Object step; "  {0:HH:mm:ss}  {1:N2} / {2:N2}  ({3} samples)" -f $_.Group[0].t, ($x | Measure-Object -Average).Average, ($x | Measure-Object -Maximum).Maximum, $x.Count }
    }
}
$fr = @(Import-Csv "$d\frames.csv" | Where-Object { $_.fixed_tick -ne "" } | ForEach-Object { [pscustomobject]@{ t = [datetime]::Parse($_.wall_utc).ToUniversalTime(); f = [int64]$_.frame; e = [double]$_.elapsed_real; k = [int64]$_.fixed_tick } })
if ($fr.Count -ge 2) {
    "engine.status samples {0}: frames {1}-{2}, fixed ticks {3}-{4}: {5:N2} fixed steps a frame over the run; {6:N1} fps" -f $fr.Count, $fr[0].f, $fr[-1].f, $fr[0].k, $fr[-1].k, (($fr[-1].k - $fr[0].k) / [math]::Max(1, $fr[-1].f - $fr[0].f)), (($fr[-1].f - $fr[0].f) / [math]::Max(0.001, $fr[-1].e - $fr[0].e))
    "per 5 s (fixed steps a frame, fps):"
    $fr | Group-Object { [int](($_.t - $fr[0].t).TotalSeconds / 5) } | ForEach-Object { $g = $_.Group; if ($g.Count -ge 2) { "  {0:HH:mm:ss}  {1:N2} steps/frame, {2:N1} fps" -f $g[0].t, (($g[-1].k - $g[0].k) / [math]::Max(1, $g[-1].f - $g[0].f)), (($g[-1].f - $g[0].f) / [math]::Max(0.001, $g[-1].e - $g[0].e)) } }
}
"==== physics reports (PHYSICS_REPORT)"
for ($j = 0; $j -lt $L.Count; $j++) {
    if ($L[$j] -match 'engine_physics::report') { "-- " + $L[$j].Substring(0, [math]::Min(400, $L[$j].Length)); for ($k = $j + 1; $k -lt $L.Count -and $L[$k] -notmatch '^20\d\d-'; $k++) { "   " + $L[$k] } }
}
"==== tour lines"
$L | Where-Object { $_ -match 'showcase::tour' } | ForEach-Object { $s = $_ -replace '^(\S+)Z\s+INFO\s+', '$1 '; $s.Substring(0, [math]::Min(240, $s.Length)) }
