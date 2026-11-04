param([string]$Run, [string]$Title)
# Every round of a --loops run, every column (21b's table printed medians for rounds 2-n).
[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\a9b51fd2-8307-4505-9047-44a5b14953cc\scratchpad\r26"
& "$sp\analyze21b.ps1" -Run $Run | Out-Null
$rows = @(); foreach ($x in (Get-Content "$sp\$Run\summary.json" -Raw | ConvertFrom-Json)) { $rows += $x }
$n = ($rows | Measure-Object round -Maximum).Maximum
foreach ($r in 1..$n) {
    "=== $Title, round $r of $n (last 20 s of each 25 s dwell" + $(if ($r -eq 1) { "; * = first stop, its dwell started before the first frames' loading" } else { "" }) + ")"
    "stop | fps | 1% low (0.5 s samples) | worst frame ms (5 s profiles) | frame ms | systems busy / one at a time ms | render_system ms (max) | GPU ms | GPU busy % | cores busy | top 5 ran alone (ms/frame)"
    $first = $true
    foreach ($x in ($rows | Where-Object { $_.round -eq $r })) {
        $gb = if ($x.gpu_busy -ne $null) { "{0:N0}" -f $x.gpu_busy } else { "n/a" }
        "{0}{1} | {2:N1} | {3:N1} | {4:N1} | {5:N0} | {6} / {7} | {8} | {9} | {10} | {11} | {12}" -f $x.stop, $(if ($first -and $r -eq 1) { "*" } else { "" }), $x.fps, $x.low1, $x.worst, $x.frame_ms, $x.sys_busy, $x.one_at_a_time, $x.render, $x.gpu_ms, $gb, $x.cores, $x.alone
        $first = $false
    }
}

