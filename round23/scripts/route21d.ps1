param([string]$Run, [int]$UtcOffsetH = 1)
# A --route run: each leg's wall-clock span (from the "route: ... done" lines), the 5 s frame and
# render profiles that report inside it (fps, worst, schedules, slowest / ran-alone systems, GPU
# passes total), GPU busy (nvidia-smi), then the worst frames of route.csv and every
# "slow render frame" line.
[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\22c660b7-08e6-4a7e-aff7-525fad477790\scratchpad"
$d = "$sp\$Run"; $L = [IO.File]::ReadAllLines("$d\stderr.log")
function TS($line) { if ($line -match '^(20\d\d-\d\d-\d\dT[\d:.]+)Z') { return [datetime]::Parse($Matches[1]) } }
$gpu = @(Get-Content "$d\nvidia_dmon.txt" | Where-Object { $_ -match '^\s*\d{8}\s+\d\d:\d\d:\d\d\s+\d+\s+\d+' } | ForEach-Object { $c = ($_.Trim() -split '\s+'); [pscustomobject]@{ t = [datetime]::ParseExact("$($c[0]) $($c[1])", "yyyyMMdd HH:mm:ss", $null).AddHours(-$UtcOffsetH); sm = [double]$c[3] } })
$legs = @(); $start = $null
for ($i = 0; $i -lt $L.Count; $i++) {
    if ($L[$i] -match 'route: and the view from the plaza after') { $start = TS $L[$i] }
    elseif ($L[$i] -match 'route: (.+) done at ([\d.]+) s, (\d+) m up') { $t = TS $L[$i]; $legs += [pscustomobject]@{ name = $Matches[1]; from = $start; to = $t; route_s = $Matches[2]; up = $Matches[3] }; $start = $t }
}
$profs = @(); $rprofs = @()
for ($j = 0; $j -lt $L.Count; $j++) {
    if ($L[$j] -match 'engine_core::profile: frame profile') { $profs += [pscustomobject]@{ t = (TS $L[$j]); i = $j } }
    elseif ($L[$j] -match 'engine_render::perf: render profile') { $rprofs += [pscustomobject]@{ t = (TS $L[$j]); i = $j } }
}
function Block([int]$i) { $out = @(); for ($j = $i + 1; $j -lt $L.Count -and $L[$j] -notmatch '^20\d\d-'; $j++) { $out += $L[$j] }; return $out }
"route start (after the plaza settle): {0:HH:mm:ss.fff} UTC" -f $legs[0].from
"leg | wall s | ends at route s / m up | GPU busy % | frame profiles reporting in the leg (each covers the 5 s before it)"
foreach ($g in $legs) {
    $gb = @($gpu | Where-Object { $_.t -ge $g.from.AddSeconds(-0.5) -and $_.t -le $g.to.AddSeconds(0.5) }); $gbs = if ($gb.Count) { "{0:N0}" -f ($gb | Measure-Object sm -Average).Average } else { "n/a" }
    "== {0} | {1:N1} s | {2} s / {3} m | GPU busy {4}" -f $g.name, ($g.to - $g.from).TotalSeconds, $g.route_s, $g.up, $gbs
    foreach ($p in ($profs | Where-Object { $_.t -gt $g.from -and $_.t -le $g.to.AddSeconds(2.5) })) {
        $b = Block $p.i; $sec = ""; $sched = @(); $slow = @(); $alone = @()
        foreach ($s in $b) { $x = ($s -replace '\s{2,}', ' ').Trim()
            if ($x -match '^schedule ms/frame') { $sec = "sc" } elseif ($x -match '^slowest systems') { $sec = "s" } elseif ($x -match '^ran alone') { $sec = "a" } elseif ($x -match '^(inside systems|ordering floor)') { $sec = "x" } elseif (-not $x) { }
            elseif ($sec -eq "sc" -and $sched.Count -lt 3 -and $x -match '^(\w+) ([\d.]+) ([\d.]+)$') { $sched += "$($Matches[1]) $($Matches[2]) (x$($Matches[3]))" }
            elseif ($sec -eq "s" -and $slow.Count -lt 6 -and $x -match '^(\S+) ([\d.]+) ([\d.]+) ([\d.]+)$') { $slow += ("{0} {1}/max {2}" -f ($Matches[1] -replace '^engine_', ''), $Matches[2], $Matches[3]) }
            elseif ($sec -eq "a" -and $alone.Count -lt 4 -and $x -match '^(\S+) ([\d.]+)$') { $alone += ("{0} {1}" -f ($Matches[1] -replace '^engine_', ''), $Matches[2]) } }
        $rp = @($rprofs | Where-Object { [math]::Abs(($_.t - $p.t).TotalSeconds) -lt 3 }); $gms = ""
        if ($rp.Count) { $rb = Block $rp[0].i; $gl = $rb | Where-Object { $_ -match '^GPU passes\s+([\d.]+)' } | Select-Object -First 1; if ($gl -match '([\d.]+) ms/frame') { $gms = $Matches[1] }; $cl = $rb | Where-Object { $_ -match '^render CPU steps\s+([\d.]+)' } | Select-Object -First 1; if ($cl -match '([\d.]+) ms/frame') { $gms += " (render CPU $($Matches[1]))" } }
        "  {0:HH:mm:ss} {1}" -f $p.t, (($b[0] -replace '\s{2,}', ' ').Trim())
        "    {0}; schedules: {1}" -f (($b[1] -replace '\s{2,}', ' ').Trim()), ($sched -join ", ")
        "    slowest: " + ($slow -join ", ")
        "    ran alone: " + ($alone -join ", ") + "; GPU passes $gms"
    }
}
""
"-- worst 25 frames in route.csv (frame_ms, leg, route s, m up, render CPU ms, terrain ms, patches missing, RAM MB)"
$csv = Import-Csv "$d\png\route\route.csv"
$csv | Sort-Object { [double]$_.frame_ms } -Descending | Select-Object -First 25 | ForEach-Object { "  {0,7} ms  {1,-16} {2,7} s {3,7} m  render CPU {4,7}  terrain {5,6}  missing {6,4}  RAM {7}" -f $_.frame_ms, $_.leg, $_.t_s, $_.altitude_m, $_.render_cpu_ms, $_.terrain_ms, $_.patches_missing, $_.ram_mb }
"-- frames over 33 / 50 / 100 ms per leg (route.csv)"
$csv | Group-Object leg | ForEach-Object { $ms = $_.Group | ForEach-Object { [double]$_.frame_ms }
    "  {0,-16} {1,4} frames: over 33 {2,4}, over 50 {3,4}, over 100 {4,3}; median {5:N1} ms; RAM {6:N0}-{7:N0} MB" -f $_.Name, $ms.Count, @($ms | Where-Object { $_ -gt 33 }).Count, @($ms | Where-Object { $_ -gt 50 }).Count, @($ms | Where-Object { $_ -gt 100 }).Count, (($ms | Sort-Object)[[int]($ms.Count / 2)]),
        ($_.Group | ForEach-Object { [double]$_.ram_mb } | Measure-Object -Minimum).Minimum, ($_.Group | ForEach-Object { [double]$_.ram_mb } | Measure-Object -Maximum).Maximum }
"-- slow render frame lines (stderr)"
$L | Where-Object { $_ -match 'slow render frame' } | ForEach-Object { $s = ($_ -replace '^(\S+)Z .*slow render frame: ', '$1 '); $s.Substring(0, [math]::Min(260, $s.Length)) }
