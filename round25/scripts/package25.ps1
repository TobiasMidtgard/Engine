param([string]$Dest)
# Lay the round 25 runs out under $Dest\round25\<step>\ as round 24 did: the README and analysis files, frames.csv,
# timeline.txt, nvidia_dmon.txt, jpg\, dumps\ (the render.cpu.* dumps every 5 s), and logs.zip with the big logs.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\caa7f13c-78a1-4b4e-9209-01cfa307faa4\scratchpad\r25"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$map = [ordered]@{ "a_plaza_plain" = "a-plaza-plain"; "b_loops" = "b-loops-ultra"; "c_night" = "c-night-moon" }
$logs = "stderr.log", "engine-profile.txt", "sampled_stats.jsonl", "fast.jsonl", "fast_offframe.jsonl", "apis.jsonl", "frames_extra.csv"
$skip = "stdout.log", "stop.flag", "fast_go.flag", "fast_halt.flag"
foreach ($run in $map.Keys) {
    $src = "$sp\$run"; if (-not (Test-Path $src)) { "missing $run"; continue }
    $dst = "$Dest\round25\$($map[$run])"
    if (Test-Path $dst) { Remove-Item -Recurse -Force $dst }
    New-Item -ItemType Directory -Force $dst | Out-Null
    $zipFiles = @()
    foreach ($f in Get-ChildItem $src -File) {
        if ($skip -contains $f.Name -or $f.Name -like "*_job*.txt") { continue }
        if ($f.Name -like "stats_dump_*.json") { New-Item -ItemType Directory -Force "$dst\dumps" | Out-Null; Copy-Item $f.FullName "$dst\dumps"; continue }
        if ($logs -contains $f.Name -or ($f.Extension -eq ".json" -and $f.Name -ne "summary.json")) { $zipFiles += $f; continue }
        Copy-Item $f.FullName $dst
    }
    if (Test-Path "$src\crops") { Copy-Item "$src\crops" "$dst\crops" -Recurse }
    $jpgs = @(Get-ChildItem "$src\jpg" -File -ErrorAction SilentlyContinue)
    if ($jpgs.Count -eq 1) { Copy-Item $jpgs[0].FullName $dst } elseif ($jpgs.Count) { Copy-Item "$src\jpg" "$dst\jpg" -Recurse }
    $zip = "$dst\logs.zip"
    $z = [System.IO.Compression.ZipFile]::Open($zip, "Create")
    try { foreach ($f in $zipFiles) { [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($z, $f.FullName, $f.Name, "Optimal") | Out-Null } }
    finally { $z.Dispose() }
    "{0,-26} {1,3} files, logs.zip {2:N1} MB ({3} files)" -f $map[$run], @(Get-ChildItem $dst -Recurse -File).Count, ((Get-Item $zip).Length / 1MB), $zipFiles.Count
}
