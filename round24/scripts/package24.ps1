param([string]$Dest)
# Lay the round 24 runs out under $Dest\round24\<step>\ as round 23 did: the README and analysis files, frames.csv,
# timeline.txt, nvidia_dmon.txt, jpg\, dumps\ (the render.cpu.* dumps every 5 s), and logs.zip with the big logs.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\45d985d3-0d66-4e84-94ac-e37b77e235f5\scratchpad\r24"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$map = [ordered]@{ "a1_plaza_plain" = "a1-plaza-plain"; "a2_plaza_sets" = "a2-plaza-fixed-sync-sets"; "b_loops" = "b-loops-ultra"; "c_night" = "c-night-moon" }
$logs = "stderr.log", "engine-profile.txt", "sampled_stats.jsonl", "fast.jsonl", "fast_offframe.jsonl", "apis.jsonl"
$skip = "stdout.log", "stop.flag"
foreach ($run in $map.Keys) {
    $src = "$sp\$run"; if (-not (Test-Path $src)) { "missing $run"; continue }
    $dst = "$Dest\round24\$($map[$run])"
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
