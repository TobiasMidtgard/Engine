param([string]$Dest)
# Lay the round 23 runs out under $Dest\round23\<step>\ as round 22 did: the README and analysis files, frames.csv,
# timeline.txt, nvidia_dmon.txt, jpg\ (route.txt and route.csv from png\route), and logs.zip with the big logs.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\22c660b7-08e6-4a7e-aff7-525fad477790\scratchpad"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$map = [ordered]@{
    "s1_plaza" = "1-plaza-render-thread"; "s2_route" = "2-route-full"; "s2_route_up" = "2-route-up"
    "s3_loops" = "3a-loops-ultra"; "s3_loops_scale07" = "3b-render-scale-0.7"; "s4_night" = "4-night"
}
$logs = "stderr.log", "engine-profile.txt", "sampled_stats.jsonl", "fast.jsonl", "fast_offframe.jsonl", "apis.jsonl"
$skip = "stdout.log", "stop.flag"
foreach ($run in $map.Keys) {
    $src = "$sp\$run"; if (-not (Test-Path $src)) { "missing $run"; continue }
    $dst = "$Dest\round23\$($map[$run])"
    if (Test-Path $dst) { Remove-Item -Recurse -Force $dst }
    New-Item -ItemType Directory -Force $dst | Out-Null
    $zipFiles = @()
    foreach ($f in Get-ChildItem $src -File) {
        if ($skip -contains $f.Name -or $f.Name -like "*_job*.txt") { continue }
        if ($logs -contains $f.Name -or ($f.Extension -eq ".json" -and $f.Name -ne "summary.json")) { $zipFiles += $f; continue }
        Copy-Item $f.FullName $dst
    }
    foreach ($f in "route.txt", "route.csv") { if (Test-Path "$src\png\route\$f") { Copy-Item "$src\png\route\$f" $dst } }
    if (Test-Path "$src\crops") { Copy-Item "$src\crops" "$dst\crops" -Recurse }
    $jpgs = @(Get-ChildItem "$src\jpg" -File -ErrorAction SilentlyContinue)
    if ($jpgs.Count -eq 1) { Copy-Item $jpgs[0].FullName $dst } elseif ($jpgs.Count) { Copy-Item "$src\jpg" "$dst\jpg" -Recurse }
    $zip = "$dst\logs.zip"
    $z = [System.IO.Compression.ZipFile]::Open($zip, "Create")
    try { foreach ($f in $zipFiles) { [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($z, $f.FullName, $f.Name, "Optimal") | Out-Null } }
    finally { $z.Dispose() }
    "{0,-22} {1,3} files, logs.zip {2:N1} MB ({3} files)" -f $map[$run], @(Get-ChildItem $dst -Recurse -File).Count, ((Get-Item $zip).Length / 1MB), $zipFiles.Count
}
