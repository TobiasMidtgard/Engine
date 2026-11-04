param([string]$Run)
# Step 3's extras for a --loops run: physics_simulate's row in each frame profile of each stop's
# window, the softbody yard line, and per stop (the round 1 snapshot at 20 s) the extra views and
# the terrain's render CPU steps and GPU pass.
[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\a9b51fd2-8307-4505-9047-44a5b14953cc\scratchpad\r26"
$d = "$sp\$Run"; $L = [IO.File]::ReadAllLines("$d\stderr.log")
"-- physics_simulate in each 5 s frame profile inside the stop's window (ms/frame, max ms, runs/frame)"
$loops = @(); for ($i = 0; $i -lt $L.Count; $i++) { if ($L[$i] -match '^(\S+)Z .*showcase::loops: loop (\d+)/\d+ at (\w+):') { $loops += [pscustomobject]@{ t = [datetime]::Parse($Matches[1]); r = $Matches[2]; k = $Matches[3] } } }
$profs = @(); for ($j = 0; $j -lt $L.Count; $j++) { if ($L[$j] -match '^(\S+)Z .*engine_core::profile: frame profile') { $profs += [pscustomobject]@{ t = [datetime]::Parse($Matches[1]); i = $j } } }
foreach ($v in $loops) { $s = $v.t.AddSeconds(-20)
    $rows = @(); foreach ($pf in ($profs | Where-Object { $_.t -gt $s.AddSeconds(4.5) -and $_.t -le $v.t.AddSeconds(0.5) })) {
        for ($m = $pf.i + 1; $m -lt $pf.i + 150 -and $m -lt $L.Count; $m++) { if ($L[$m] -match '^engine_physics::physics_simulate\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)') { $rows += ("{0} / {1} / {2}" -f $Matches[1], $Matches[2], $Matches[3]); break } } }
    "round {0} {1,-9}: {2}" -f $v.r, $v.k, ($rows -join " | ") }
"-- softbody yard"
$L | Where-Object { $_ -match 'softbody yard: made' } | ForEach-Object { $_ -replace '^.*INFO ', '' }
"-- per stop (engine.stats at 20 s into round 1's dwell; overview's in round 2)"
foreach ($f in (Get-ChildItem "$d\stats_render_*.json" | Sort-Object LastWriteTime)) {
    $k = $f.BaseName -replace '^stats_render_', ''
    $m = (Get-Content $f.FullName -Raw | ConvertFrom-Json).result.metrics
    $t = (Get-Content "$d\stats_terrain_$k.json" -Raw | ConvertFrom-Json).result.metrics
    $cpu = @($m.PSObject.Properties | Where-Object { $_.Name -like 'render.cpu.terrain*' -and [double]$_.Value -ge 0.005 } | ForEach-Object { "{0} {1:N2}" -f ($_.Name -replace '^render\.cpu\.terrain: ', ''), [double]$_.Value })
    "{0,-9}: prepare terrain {1:N2} ms (inside: {2}); GPU scene: terrain {3:N2} ms; extra views {4} ({5:N2} ms CPU, {6:N2} ms GPU); patches drawn {7}, resident {8}, building {9}, missing {10}" -f $k,
        [double]$m.'render.cpu.prepare terrain', ($cpu -join ", "), [double]$m.'render.gpu.scene: terrain', $m.'render.extra_views', [double]$m.'render.extra_views_ms', [double]$m.'render.extra_views_gpu_ms',
        $t.'terrain.patches_drawn', $t.'terrain.patches_resident', $t.'terrain.patches_building', $t.'terrain.patches_missing'
}

