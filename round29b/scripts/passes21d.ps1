param([string]$Run, [string]$Mode = "loops", [double]$WindowS = 20, [int]$Top = 16)
# Per-pass GPU table: for each window (a --loops stop's last $WindowS s before its "loop" line, or a
# `stops` stay's last $WindowS s before END), the last render profile inside it ("GPU passes", 5 s
# averages): one compact row of the main passes, then that profile's passes sorted, top $Top.
[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\a9b51fd2-8307-4505-9047-44a5b14953cc\scratchpad\r29b"
$d = "$sp\$Run"; $L = [IO.File]::ReadAllLines("$d\stderr.log")
function TS($line) { if ($line -match '^(20\d\d-\d\d-\d\dT[\d:.]+)Z') { return [datetime]::Parse($Matches[1]) } }
$day = (TS $L[0]).ToString("yyyy-MM-dd")
$wins = @()
if ($Mode -eq "loops") {
    for ($i = 0; $i -lt $L.Count; $i++) { if ($L[$i] -match 'showcase::loops: loop (\d+)/\d+ at (\w+):') { $t = TS $L[$i]; $wins += [pscustomobject]@{ label = "round $($Matches[1]) $($Matches[2])"; start = $t.AddSeconds(-$WindowS); end = $t } } }
} else {
    # A stay: render profiles from 10 s after arriving to its END (they come every ~10 s).
    $at = @{}
    foreach ($x in (Get-Content "$d\timeline.txt")) {
        if ($x -match '^(\d\d:\d\d:\d\d\.\d+)Z\s+\S+\s+AT (\w+):') { $at[$Matches[2]] = [datetime]::Parse("${day}T$($Matches[1])") }
        elseif ($x -match '^(\d\d:\d\d:\d\d\.\d+)Z\s+\S+\s+END (\w+):') { $e = [datetime]::Parse("${day}T$($Matches[1])"); $wins += [pscustomobject]@{ label = $Matches[2]; start = $at[$Matches[2]].AddSeconds(5.5); end = $e } } }
}
$rprofs = @(); for ($j = 0; $j -lt $L.Count; $j++) { if ($L[$j] -match 'engine_render::perf: render profile') { $rprofs += [pscustomobject]@{ t = (TS $L[$j]); i = $j } } }
$main = "scene", "scene: terrain", "scene: meshes", "compute meshes", "prepass", "shadows", "sky occlusion", "compute weather clouds", "volumetric fog", "environment", "bounce light", "transparent", "probe gi", "world reflections", "reflections", "ambient occlusion", "atmosphere", "anti-aliasing", "bloom"
foreach ($v in $wins) {
    $rp = @($rprofs | Where-Object { $_.t -gt $v.start.AddSeconds(4.5) -and $_.t -le $v.end.AddSeconds(0.5) })
    if (-not $rp.Count) { "{0,-18} no render profile in the window" -f $v.label; continue }
    $sec = ""; $vals = [ordered]@{}; $total = ""; $cpu = ""
    for ($j = $rp[-1].i + 1; $j -lt $L.Count -and $L[$j] -notmatch '^20\d\d-'; $j++) {
        $s = $L[$j]
        if ($s -match '^GPU passes\s+([\d.]+)') { $sec = "g"; $total = $Matches[1] }
        elseif ($s -match '^render CPU steps\s+([\d.]+)') { $sec = "c"; $cpu = $Matches[1] }
        elseif ($s -match '^\S') { $sec = "" }
        elseif ($sec -eq "g" -and $s -match '^\s+(.+?)\s+([\d.]+)$') { $vals[$Matches[1]] = [double]$Matches[2] }
    }
    $cols = @($main) + @($vals.Keys | Where-Object { $_ -match 'aerial' -and $main -notcontains $_ })
    $row = ($cols | ForEach-Object { "{0} {1:N2}" -f $_, $(if ($vals.Contains($_)) { $vals[$_] } else { 0 }) }) -join "  "
    "{0,-18} {1:HH:mm:ss} GPU total {2}  render CPU {3}  |  {4}" -f $v.label, $rp[-1].t, $total, $cpu, $row
    $sorted = $vals.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First $Top
    "    sorted: " + (($sorted | ForEach-Object { "{0} {1:N2}" -f $_.Key, $_.Value }) -join ", ")
}
