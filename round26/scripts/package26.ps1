param([string]$Dest)
# Lay the round 26 runs out under $Dest\round26\<step>\ as round 25 did: the README and analysis files, frames.csv,
# timeline.txt, nvidia_dmon.txt, jpg\, dumps\ (the render.cpu.* dumps every 5 s), logs.zip with the big logs, and
# traces\ with each engine.trace as .json.gz (made by gz26.py) and its trace-summary.py text.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\a9b51fd2-8307-4505-9047-44a5b14953cc\scratchpad\r26"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$map = [ordered]@{ "a_plaza_plain" = "a-plaza-plain"; "b_loops" = "b-loops-ultra" }
$logs = "stderr.log", "engine-profile.txt", "sampled_stats.jsonl", "fast.jsonl", "fast_offframe.jsonl", "apis.jsonl", "frames_extra.csv"
$skip = "stdout.log", "stop.flag", "fast_go.flag", "fast_halt.flag"
foreach ($run in $map.Keys) {
    $src = "$sp\$run"; if (-not (Test-Path $src)) { "missing $run"; continue }
    $dst = "$Dest\round26\$($map[$run])"
    if (Test-Path $dst) { Remove-Item -Recurse -Force $dst }
    New-Item -ItemType Directory -Force $dst | Out-Null
    $zipFiles = @()
    foreach ($f in Get-ChildItem $src -File) {
        if ($skip -contains $f.Name -or $f.Name -like "*_job*.txt") { continue }
        if ($f.Name -like "trace_*.json") { continue }
        if ($f.Name -like "trace_*.json.gz" -or $f.Name -like "trace_*_summary.txt") { New-Item -ItemType Directory -Force "$dst\traces" | Out-Null; Copy-Item $f.FullName "$dst\traces"; continue }
        if ($f.Name -like "stats_dump_*.json") { New-Item -ItemType Directory -Force "$dst\dumps" | Out-Null; Copy-Item $f.FullName "$dst\dumps"; continue }
        if ($logs -contains $f.Name -or ($f.Extension -eq ".json" -and $f.Name -ne "summary.json")) { $zipFiles += $f; continue }
        Copy-Item $f.FullName $dst
    }
    $jpgs = @(Get-ChildItem "$src\jpg" -File -ErrorAction SilentlyContinue)
    if ($jpgs.Count -eq 1) { Copy-Item $jpgs[0].FullName $dst } elseif ($jpgs.Count) { Copy-Item "$src\jpg" "$dst\jpg" -Recurse }
    $zip = "$dst\logs.zip"
    $z = [System.IO.Compression.ZipFile]::Open($zip, "Create")
    try { foreach ($f in $zipFiles) { [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($z, $f.FullName, $f.Name, "Optimal") | Out-Null } }
    finally { $z.Dispose() }
    "{0,-26} {1,3} files, logs.zip {2:N1} MB ({3} files)" -f $map[$run], @(Get-ChildItem $dst -Recurse -File).Count, ((Get-Item $zip).Length / 1MB), $zipFiles.Count
}
