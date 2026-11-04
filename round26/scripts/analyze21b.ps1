param([string]$Run, [int]$WindowS = 20, [int]$UtcOffsetH = 1, [string]$From = "", [string]$To = "", [string]$Label = "")
# Per stop of a --loops run (the last $WindowS s before its "loop" line), or one window
# ($From-$To, UTC HH:mm:ss.fff, for a --fly run):
# frames.csv (engine.status twice a second) -> fps, frame ms, and a 1% low from the 0.5 s
#   samples (each frame of a sample gets the sample's mean, so it is smoothed);
# the engine's own 5 s frame profiles inside the window -> worst frame (exact);
# nvidia_dmon.txt -> GPU busy (sm %); the last frame profile and render perf line in the window
# -> systems busy / one at a time, cores, render_system, ran alone top 5, GPU ms.
[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\a9b51fd2-8307-4505-9047-44a5b14953cc\scratchpad\r26"
$d = "$sp\$Run"
$L = [IO.File]::ReadAllLines("$d\stderr.log")
$frames = @(Import-Csv "$d\frames.csv" | ForEach-Object { [pscustomobject]@{ t = [datetime]::Parse($_.wall_utc).ToUniversalTime(); f = [int64]$_.frame; e = [double]$_.elapsed_real } })
$gpu = @(Get-Content "$d\nvidia_dmon.txt" | Where-Object { $_ -match '^\s*\d{8}\s+\d\d:\d\d:\d\d\s+\d+\s+\d+' } | ForEach-Object { $c = ($_.Trim() -split '\s+'); [pscustomobject]@{ t = [datetime]::ParseExact("$($c[0]) $($c[1])", "yyyyMMdd HH:mm:ss", $null).AddHours(-$UtcOffsetH); sm = [double]$c[3] } })
function TS($line) { if ($line -match '^(20\d\d-\d\d-\d\dT[\d:.]+)Z') { return [datetime]::Parse($Matches[1]) } }
$day = (TS $L[0]).ToString("yyyy-MM-dd")
$wins = @()
if ($From) { $wins += [pscustomobject]@{ round = ""; key = $Label; start = [datetime]::Parse("${day}T$From"); end = [datetime]::Parse("${day}T$To"); i = $L.Count - 1 } }
else { for ($i = 0; $i -lt $L.Count; $i++) { if ($L[$i] -match 'showcase::loops: loop (\d+)/\d+ at (\w+):') { $t = TS $L[$i]; $wins += [pscustomobject]@{ round = [int]$Matches[1]; key = $Matches[2]; start = $t.AddSeconds(-$WindowS); end = $t; i = $i } } } }
# Frame profile blocks: time, worst, and line index.
$profs = @(); for ($j = 0; $j -lt $L.Count; $j++) { if ($L[$j] -match 'engine_core::profile: frame profile') { $w = if ($L[$j + 1] -match 'worst ([\d.]+) ms') { [double]$Matches[1] } else { $null }; $profs += [pscustomobject]@{ t = (TS $L[$j]); worst = $w; i = $j; head = $L[$j + 1] } } }
$rows = @()
foreach ($v in $wins) {
    $fr = @($frames | Where-Object { $_.t -ge $v.start -and $_.t -le $v.end })
    $dts = New-Object System.Collections.Generic.List[double]; $ivals = @()
    for ($k = 1; $k -lt $fr.Count; $k++) { $df = $fr[$k].f - $fr[$k-1].f; if ($df -ge 1) { $dt = ($fr[$k].e - $fr[$k-1].e) * 1000 / $df; $ivals += $dt; for ($m = 0; $m -lt $df; $m++) { $dts.Add($dt) } } }
    $fps = $null; $avg = $null; $low = $null; $slow = $null
    if ($fr.Count -ge 3) { $nf = $fr[-1].f - $fr[0].f; $secs = $fr[-1].e - $fr[0].e; if ($nf -gt 0) { $avg = $secs * 1000 / $nf; $fps = 1000 / $avg }
        $sorted = $dts | Sort-Object; $p99 = $sorted[[math]::Min($sorted.Count - 1, [int][math]::Ceiling($sorted.Count * 0.99) - 1)]; $low = 1000 / $p99; $slow = ($ivals | Measure-Object -Maximum).Maximum }
    # Profiles reporting inside the window (each covers the 5 s before it).
    $pw = @($profs | Where-Object { $_.t -gt $v.start.AddSeconds(4.5) -and $_.t -le $v.end.AddSeconds(0.5) })
    $worst = if ($pw.Count) { ($pw | Measure-Object worst -Maximum).Maximum } else { $null }
    $g = @($gpu | Where-Object { $_.t -ge $v.start -and $_.t -le $v.end }); $gbusy = if ($g.Count) { ($g | Measure-Object sm -Average).Average } else { $null }
    $fp = if ($pw.Count) { $pw[-1].i } else { -1 }
    $pl = -1; for ($j = $v.i; $j -ge 0; $j--) { $t = TS $L[$j]; if ($t -and $t -lt $v.start) { break }; if ($L[$j] -match 'engine_render::perf: perf:') { $pl = $j; break } }
    $busy = ""; $one = ""; $render = ""; $alone = @(); $sec = ""; $fpline = ""
    if ($fp -ge 0) { for ($j = $fp + 1; $j -lt $L.Count -and $L[$j] -notmatch '^20\d\d-'; $j++) {
        $s = ($L[$j] -replace '\s{2,}', ' ').Trim()
        if ($s -match 'frames in') { $fpline = $s }
        elseif ($s -match '^systems busy') { $busy = $s } elseif ($s -match '^one system at a time') { $one = $s }
        elseif ($s -match '^slowest systems') { $sec = "s" } elseif ($s -match '^ran alone') { $sec = "a" } elseif ($s -match '^(inside systems|ordering floor|schedule)') { $sec = "x" }
        elseif ($sec -eq "s" -and $s -match '^engine_render::render_system ([\d.]+) ([\d.]+)') { $render = "$($Matches[1]) (max $($Matches[2]))" }
        elseif ($sec -eq "a" -and $s -and $alone.Count -lt 5) { $alone += ($s -replace '^engine_', '' -replace '^showcase::', 'showcase::') }
    } }
    $gms = if ($pl -ge 0 -and $L[$pl] -match 'GPU ([\d.]+) ms') { $Matches[1] } else { "" }
    $cores = if ($busy -match '([\d.]+) of \d+ cores') { $Matches[1] } else { "" }
    $sysbusy = if ($busy -match 'systems busy ([\d.]+) ms') { $Matches[1] } else { "" }
    $onet = if ($one -match 'one system at a time ([\d.]+) ms') { $Matches[1] } else { "" }
    $rows += [pscustomobject]@{ round = $v.round; stop = $v.key; fps = $fps; low1 = $low; slowest_half_s_ms = $slow; worst = $worst; frame_ms = $avg; samples = $fr.Count; profiles = $pw.Count
        sys_busy = $sysbusy; one_at_a_time = $onet; render = $render; gpu_ms = $gms; gpu_busy = $gbusy; cores = $cores; alone = ($alone -join ", "); perf = $(if ($pl -ge 0) { $L[$pl] -replace '^.*perf: perf: ', '' } else { "" }) }
}
$rows | ConvertTo-Json -Depth 3 | Set-Content "$d\summary.json"
$rows | ForEach-Object { "{0} {1,-10} fps {2,5:N1} low1 {3,5:N1} worst {4,6:N1} frame {5,6:N1} busy {6}/{7} render {8} gpu {9} busy% {10,3:N0} cores {11} | {12}" -f $_.round, $_.stop, $_.fps, $_.low1, $_.worst, $_.frame_ms, $_.sys_busy, $_.one_at_a_time, $_.render, $_.gpu_ms, $_.gpu_busy, $_.cores, $_.alone }

