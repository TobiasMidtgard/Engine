param(
    [ValidateSet("loops", "stops", "route", "tour", "night")] [string]$Mode,
    [string]$OutName,
    [string]$Quality = "ultra",
    [int]$Port = 7931,
    [string]$Spec = "",          # loops: places[:rounds]; stops: "key:secs,walk:secs,..."; mem: place
    [string]$Size = "",          # "800x450": --size
    [string]$SampleStats = "",   # a prefix the sampler also reads with engine.stats each poll
    [string]$Apis = "",          # bridge methods (no params) the sampler also calls every $ApiEvery polls -> apis.jsonl
    [int]$ApiEvery = 2,
    [string]$FastPrefix = "",    # engine.stats prefixes (comma-separated), each read back to back (about once a frame) on its own
                                 # connection -> fast.jsonl for the first, fast_<prefix>.jsonl for the others
    [string]$UpKm = "",          # route: altitudes (km, comma-separated) for a bridge screenshot each on the way up (space.bodies polled every frame)
    [string]$Hours = "22.5",     # night: hours (comma-separated) to set with showcase.time, each visiting the $Spec places
    [double]$NightSettle = 10,   # night: seconds at a place before its picture
    [hashtable]$ExtraEnv = @{},
    [int]$MaxMinutes = 30,
    [double]$HoldMin = 10,       # mem: minutes to hold
    [double]$MaxPrivateGB = 30,
    [double]$MinFreeGB = 10,
    [double]$Settle = 5,         # gi: seconds after the jump before the 20 s window
    [switch]$Fly,                # tour: add --fly
    [switch]$NoProfile,          # leave --profile off (tour: ENGINE_PROFILE in ExtraEnv instead)
    [double]$DumpEvery = 0,      # stops: also save engine.stats $DumpPrefix every $DumpEvery s while standing (stats_dump_<key>_<s>.json)
    [string]$DumpPrefix = "render.cpu.",
    [string]$FastGoAfter = "",   # loops: start the fast samplers only once this stop's picture is taken (empty: from the start)
    [double]$FastHaltAfter = 4,  # loops: ... and halt them this many s after that stop's loop line (the arrival at the next)
    [double]$TraceAt = -1,       # stops: engine.trace $TraceFrames frames this many s after the jump (right after that s's dump) -> trace_<key>.json
    [int]$TraceFrames = 120,
    [string]$TraceStops = "",    # loops: engine.trace $TraceStopFrames frames $TraceAfter s after arriving at each of these stops -> trace_<stop>.json
    [double]$TraceAfter = 10,
    [int]$TraceStopFrames = 60,
    [string]$ArrivalTrace = "",  # loops: engine.trace $ArrivalFrames frames armed just before the jump to this stop -> trace_<stop>_arrival.json
    [int]$ArrivalFrames = 30,
    [double]$ArrivalArm = 24.96, # loops: ... armed this many s after the previous stop's loop line's own log time (the stops are 25 s
                                 # apart from the arrival, the lines ~25.03 s apart), so the trace holds the jump frame
    [int]$ArriveTraceFrames = 20,      # stops "arrive-<place>:secs": the arrival trace's frames
    [int]$ArriveStatsFrames = 20,      # ... and how many engine.stats answers (one a frame) right after the goto
    [string]$ArriveStatsPrefix = "render.gi"
)
# Round 28 PC driver: round 27's, plus a stops entry "arrive-<place>:secs": an engine.trace answered just before the
# goto (so the arrival frame is the first recorded), then one engine.stats answer a frame for the first frames there.
# Round 27's: round 26's, plus $TraceAt in a stops stand without dumps (stand to it, trace, stand the rest).
# Round 26's: round 25's, plus engine.trace (the bridge's Chrome trace of the next N frames): at a set second of
# a stops stand, a set time after arriving at loops stops, and one armed just before a loops jump. One trace at a time:
# each waits for the log's "trace of N frames written to ..." line of the one before. Loops lines are read from the
# log's tail every 50 ms (round 25: the whole log every 200 ms).
# Round 25's: round 24's, plus a fast sampler that can run only from one loops stop's picture to a few s
# after its loop line ($FastGoAfter, $FastHaltAfter: the arrival at the next stop).
# Round 24's: round 23's, plus a stats dump every $DumpEvery s during a stops stand.
# Round 23's: round 22's, plus several fast prefixes, route shots at set altitudes, a night mode, and the sampler
# logs fps and frame_ms (22's was 21d's plus route sampling; 21c's, plus route and tour from 21b). Plain showcase.exe in the window it opens;
# input only through the bridge.
# - nvidia-smi dmon logs GPU use beside the game.
# - a background job polls engine.status twice a second (frame + real elapsed time + entities),
#   and engine.stats for $SampleStats if given.
# - Stops at once and kills the game if the user touches the PC.
$ErrorActionPreference = "Continue"
[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\a9b51fd2-8307-4505-9047-44a5b14953cc\scratchpad\r28"
. "$sp\idle.ps1"
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class G22 {
    public delegate bool P(IntPtr h, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] public static extern bool EnumWindows(P cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    public static IntPtr GameWindow(uint pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p);
            if (p == pid && IsWindowVisible(h)) { var t = new System.Text.StringBuilder(64); GetWindowText(h, t, 64); if (t.ToString().StartsWith("Engine")) { found = h; return false; } }
            return true; }, IntPtr.Zero);
        return found;
    }
}
"@ -ErrorAction SilentlyContinue
$repo = "C:\Users\Tobit\Documents\ProgrammingProjects\Engine"
$out = "$sp\$OutName"; if (Test-Path $out) { Remove-Item -Recurse -Force $out }; New-Item -ItemType Directory -Force $out | Out-Null
$log = "$out\timeline.txt"
$script:t0 = Get-Date
function Note([string]$s) { $line = "{0:HH:mm:ss.fff}Z {1,8:F2}  {2}" -f (Get-Date).ToUniversalTime(), ((Get-Date) - $script:t0).TotalSeconds, $s; Add-Content $log $line; Write-Host $line }
$script:userBack = $false
function Check-User { if (-not $script:userBack -and (Test-UserBack $script:t0)) { $script:userBack = $true; Note "USER INPUT DETECTED - stopping"; taskkill /IM showcase.exe /F | Out-Null }; return -not $script:userBack }
$script:hung = $false; $script:hungCheck = Get-Date; $script:startupNoted = $false
function Test-Hung {
    if ($script:hung) { return $true }
    if (((Get-Date) - $script:hungCheck).TotalSeconds -lt 2) { return $false }
    $script:hungCheck = Get-Date
    foreach ($m in @(Select-String -Path "$out\stderr.log" -Pattern "frame (\d+) has not finished for" -ErrorAction SilentlyContinue)) {
        $fr = [int64]$m.Matches[0].Groups[1].Value
        if ($fr -gt 3) { $script:hung = $true; Note "HANG: the engine's watchdog fired on frame $fr - stopping this run"; break }
        elseif (-not $script:startupNoted) { $script:startupNoted = $true; Note "watchdog named startup frame $fr (not counted as a hang)" }
    }
    return $script:hung
}
$script:memStop = $false; $script:memCheck = Get-Date
function Test-Mem {
    if ($script:memStop) { return $true }
    if (((Get-Date) - $script:memCheck).TotalSeconds -lt 5) { return $false }
    $script:memCheck = Get-Date
    $q = Get-Process -Id $p.Id -ErrorAction SilentlyContinue; $free = (Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1MB
    if ($q -and ($q.PrivateMemorySize64 / 1GB -gt $MaxPrivateGB -or $free -lt $MinFreeGB)) { $script:memStop = $true; Note ("MEMORY GUARD: " + (Mem) + " - stopping this run") }
    return $script:memStop
}
function Pause-Checked([double]$secs) {
    $end = (Get-Date).AddSeconds($secs)
    while ((Get-Date) -lt $end) { if (-not (Check-User)) { return $false }; if ($p.HasExited) { return $false }; if (Test-Hung) { return $false }; if (Test-Mem) { return $false }; Start-Sleep -Milliseconds 200 }
    return $true
}

$idle0 = Get-IdleSeconds
Note ("mode $Mode quality $Quality size '$Size' spec '$Spec'; idle before start {0:N0} s; HEAD {1}" -f $idle0, (git -C $repo rev-parse --short HEAD))
if ($idle0 -lt 300) { Note "user active in the last 5 min - not starting"; return }
if (@(Get-Process showcase -ErrorAction SilentlyContinue).Count) { Note "showcase.exe already running - killing"; taskkill /IM showcase.exe /F | Out-Null; Start-Sleep 2 }
if (@(Get-Process cargo, rustc -ErrorAction SilentlyContinue).Count) { Note "a build is running - not starting"; return }
if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) { Note "port $Port busy - stopping"; return }
foreach ($f in "screenshots", "engine-profile.txt", "census.md", "census.details.md") { if (Test-Path "$repo\$f") { Note "leftover $f in the repo - stopping (look first)"; return } }

$dmon = Start-Process "nvidia-smi" -ArgumentList "dmon -s u -d 1 -o DT -f `"$out\nvidia_dmon.txt`"" -WindowStyle Hidden -PassThru

$gameArgs = @()
switch ($Mode) {
    "loops" { $gameArgs += @("--loops", $Spec) }
    "route" { $gameArgs += "--route"; if ($Spec) { $gameArgs += $Spec } }
    "tour" { $gameArgs += @("--tour", $Spec); if ($Fly) { $gameArgs += "--fly" } }
    default { $gameArgs += "--fly" }
}
if (-not $NoProfile) { $gameArgs += "--profile" }; $gameArgs += @("--quality", $Quality)
if ($Size) { $gameArgs += @("--size", $Size) }
$env:RUST_BACKTRACE = "1"; $env:ENGINE_BRIDGE = "127.0.0.1:$Port"
foreach ($k in $ExtraEnv.Keys) { Set-Item "Env:$k" $ExtraEnv[$k] }
$p = Start-Process "$repo\target\release\showcase.exe" -ArgumentList $gameArgs -WorkingDirectory $repo `
    -RedirectStandardError "$out\stderr.log" -RedirectStandardOutput "$out\stdout.log" -PassThru
Remove-Item Env:RUST_BACKTRACE, Env:ENGINE_BRIDGE; foreach ($k in $ExtraEnv.Keys) { Remove-Item "Env:$k" }
$null = $p.Handle
Note ("started: showcase.exe " + ($gameArgs -join " ") + $(if ($ExtraEnv.Count) { "  env " + (($ExtraEnv.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join " ") } else { "" }))

$h = [IntPtr]::Zero
while (-not $p.HasExited -and ((Get-Date) - $script:t0).TotalSeconds -lt 90 -and (Check-User)) { $h = [G22]::GameWindow([uint32]$p.Id); if ($h -ne [IntPtr]::Zero) { break }; Start-Sleep -Milliseconds 50 }
function Window-Info { $c = New-Object G22+RECT; $w = New-Object G22+RECT; [G22]::GetClientRect($h, [ref]$c) | Out-Null; [G22]::GetWindowRect($h, [ref]$w) | Out-Null
    "client {0}x{1}, window {2}x{3} at ({4},{5}), maximized {6}" -f ($c.R - $c.L), ($c.B - $c.T), ($w.R - $w.L), ($w.B - $w.T), $w.L, $w.T, [G22]::IsZoomed($h) }
if ($h -ne [IntPtr]::Zero) { Note ("window visible: " + (Window-Info)) } else { Note "no window seen" }

function Mem { $q = Get-Process -Id $p.Id -ErrorAction SilentlyContinue; $os = Get-CimInstance Win32_OperatingSystem
    if ($q) { "working set {0:N0} MB, private {1:N0} MB; system free {2:N0} of {3:N0} MB" -f ($q.WorkingSet64 / 1MB), ($q.PrivateMemorySize64 / 1MB), ($os.FreePhysicalMemory / 1KB), ($os.TotalVisibleMemorySize / 1KB) } }
function Wait-Built([int]$maxS = 400) {
    while (((Get-Date) - $script:t0).TotalSeconds -lt $maxS) {
        if (-not (Check-User) -or $p.HasExited -or (Test-Hung)) { return $false }
        if (Select-String -Path "$out\stderr.log" -Pattern "showcase world built" -Quiet -ErrorAction SilentlyContinue) { Note "world built"; return $true }
        Start-Sleep -Milliseconds 250
    }
    Note "world not built by $maxS s"; return $false
}

$client = $null; $sampler = $null; $fasts = @(); $stop = "$out\stop.flag"
$routeOnly = $Mode -eq "route" -and -not ($SampleStats -or $Apis -or $FastPrefix)
if ($routeOnly) { $i = 300 }   # the route logs every frame itself: no bridge client unless asked to sample
for ($i = $i; $i -lt 300 -and -not $client -and -not $p.HasExited -and (Check-User); $i++) { try { $client = New-Object System.Net.Sockets.TcpClient("127.0.0.1", $Port) } catch { Start-Sleep -Milliseconds 200 } }
if ($routeOnly) { } elseif (-not $client) { Note "bridge never came up" } else {
    $stream = $client.GetStream(); $stream.ReadTimeout = 60000
    $wr = New-Object System.IO.StreamWriter($stream); $wr.AutoFlush = $true; $rd = New-Object System.IO.StreamReader($stream)
    Note "bridge connected"
    # engine.status twice a second on its own connection (and engine.stats for $SampleStats).
    $sampler = Start-Job -ArgumentList $Port, "$out\frames.csv", "$out\sampled_stats.jsonl", $stop, $SampleStats, $Apis, $ApiEvery, "$out\apis.jsonl" -ScriptBlock {
        param($port, $csv, $jsonl, $stop, $prefix, $apis, $apiEvery, $apiFile)
        $c = New-Object System.Net.Sockets.TcpClient("127.0.0.1", $port); $s = $c.GetStream(); $s.ReadTimeout = 60000
        $w = New-Object System.IO.StreamWriter($s); $w.AutoFlush = $true; $r = New-Object System.IO.StreamReader($s)
        $f = New-Object System.IO.StreamWriter($csv); $f.WriteLine("wall_utc,frame,elapsed_real,entities,fixed_tick,fps,frame_ms")
        $g = if ($prefix) { New-Object System.IO.StreamWriter($jsonl) } else { $null }
        $a = if ($apis) { New-Object System.IO.StreamWriter($apiFile) } else { $null }
        $n = 0; $poll = 0
        while (-not (Test-Path $stop)) {
            $n++; $poll++; $due = (Get-Date).AddMilliseconds(500)
            if ($a -and ($poll % $apiEvery -eq 1 -or $apiEvery -le 1)) { foreach ($api in $apis.Split(",")) { $n++
                try { $w.WriteLine('{"jsonrpc":"2.0","id":' + $n + ',"method":"' + $api + '","params":{}}'); $line = $r.ReadLine() } catch { $line = $null }
                if ($line) { $a.WriteLine(("{0:o} {1} {2}" -f (Get-Date).ToUniversalTime(), $api, $line)) } }
                $a.Flush() }
            try { $w.WriteLine('{"jsonrpc":"2.0","id":' + $n + ',"method":"engine.status","params":{}}'); $line = $r.ReadLine() } catch { break }
            if (-not $line) { break }
            if ($line -match '"frame":(\d+)' ) { $fr = [int64]$Matches[1]; $ent = if ($line -match '"entities":(\d+)') { $Matches[1] } else { "" }
                $ft = if ($line -match '"fixed_tick":(\d+)') { $Matches[1] } else { "" }
                $fps = if ($line -match '"fps":([0-9.eE+-]+)') { $Matches[1] } else { "" }
                $fms = if ($line -match '"frame_ms":([0-9.eE+-]+)') { $Matches[1] } else { "" }
                if ($line -match '"elapsed_real_secs":([0-9.eE+-]+)') { $f.WriteLine(("{0:o},{1},{2},{3},{4},{5},{6}" -f (Get-Date).ToUniversalTime(), $fr, $Matches[1], $ent, $ft, $fps, $fms)) } }
            if ($g) { foreach ($pre in $prefix.Split(",")) { $n++
                try { $w.WriteLine('{"jsonrpc":"2.0","id":' + $n + ',"method":"engine.stats","params":{"prefix":"' + $pre + '"}}'); $line = $r.ReadLine() } catch { $line = $null }
                if (-not $line) { break }
                $g.WriteLine(("{0:o} {1}" -f (Get-Date).ToUniversalTime(), $line)) }
                if (-not $line) { break } }
            if ($n % 10 -eq 0) { $f.Flush(); if ($g) { $g.Flush() } }
            $left = ($due - (Get-Date)).TotalMilliseconds; if ($left -gt 0) { Start-Sleep -Milliseconds ([int]$left) }
        }
        $f.Close(); if ($g) { $g.Close() }; if ($a) { $a.Close() }; $c.Close()
    }
    $fasts = @(); $go = "$out\fast_go.flag"; $halt = "$out\fast_halt.flag"
    if (-not $FastGoAfter) { New-Item $go -Force | Out-Null }
    foreach ($fp in @($FastPrefix.Split(",") | Where-Object { $_ })) {
        # engine.stats for $fp back to back: about one answer a frame, each with its frame and frame_ms.
        $ffile = if (-not $fasts.Count) { "$out\fast.jsonl" } else { "$out\fast_" + $fp.TrimEnd(".") + ".jsonl" }
        $fasts += Start-Job -ArgumentList $Port, $ffile, $stop, $fp, $go, $halt -ScriptBlock {
            param($port, $file, $stop, $prefix, $go, $halt)
            $c = New-Object System.Net.Sockets.TcpClient("127.0.0.1", $port); $s = $c.GetStream(); $s.ReadTimeout = 60000
            $w = New-Object System.IO.StreamWriter($s); $w.AutoFlush = $true; $r = New-Object System.IO.StreamReader($s)
            $g = New-Object System.IO.StreamWriter($file); $n = 0
            while (-not (Test-Path $go) -and -not (Test-Path $stop)) { Start-Sleep -Milliseconds 20 }
            while (-not (Test-Path $stop) -and -not (Test-Path $halt)) { $n++
                try { $w.WriteLine('{"jsonrpc":"2.0","id":' + $n + ',"method":"engine.stats","params":{"prefix":"' + $prefix + '"}}'); $line = $r.ReadLine() } catch { break }
                if (-not $line) { break }
                $g.WriteLine(("{0:o} {1}" -f (Get-Date).ToUniversalTime(), $line)); if ($n % 50 -eq 0) { $g.Flush() } }
            $g.Close(); $c.Close()
        }
        Note "fast sampler started ($fp -> $(Split-Path $ffile -Leaf))"
    }
}
$script:id = 0
function Call([string]$method, [string]$params = "{}") { $script:id++; $wr.WriteLine('{"jsonrpc":"2.0","id":' + $script:id + ',"method":"' + $method + '","params":' + $params + '}'); return $rd.ReadLine() }
function Shot([string]$name) { $r = Call "render.screenshot" ('{"path": "screenshots/r28/' + $name + '.png"}'); Note ("shot ${name}: " + $r) }
# The focus's altitude over the world's anchor body (space.bodies: distance minus radius).
function Anchor-Alt { $j = Call "space.bodies" | ConvertFrom-Json; $a = $j.result.anchor; $b = @($j.result.bodies | Where-Object { $_.name -eq $a })[0]; if ($b) { return [double]$b.altitude_m } else { return $null } }
function Save-Stats([string]$name, [string]$prefix = "render.") { $r = Call "engine.stats" ('{"prefix": "' + $prefix + '"}'); Set-Content "$out\stats_$name.json" $r }
# The log's new whole lines since the last call (the game writes stderr unbuffered).
$script:tailPos = 0; $script:tailRest = ""
function Read-NewLines {
    if (-not (Test-Path "$out\stderr.log")) { return @() }
    $fs = [System.IO.File]::Open("$out\stderr.log", [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try { $len = $fs.Length - $script:tailPos; if ($len -le 0) { return @() }
        $fs.Seek($script:tailPos, [System.IO.SeekOrigin]::Begin) | Out-Null; $buf = New-Object byte[] $len; $n = $fs.Read($buf, 0, $len); $script:tailPos += $n }
    finally { $fs.Close() }
    $parts = ($script:tailRest + [System.Text.Encoding]::UTF8.GetString($buf, 0, $n)) -split "`n"
    $script:tailRest = $parts[-1]
    if ($parts.Count -lt 2) { return @() }
    return @($parts[0..($parts.Count - 2)] | ForEach-Object { $_.TrimEnd("`r") })
}
# engine.trace: one at a time; $script:tracePending is the file asked for until its "written" line.
$script:tracePending = $null; $script:traceAsked = $null; $script:traceSeen = 0
function Trace-Lines([string[]]$lines) {
    foreach ($l in $lines) {
        if ($l -match 'trace of (\d+) frames written to (.+)$') { $fr = $Matches[1]; $leaf = Split-Path $Matches[2] -Leaf
            $logAt = if ($l -match '^(20\S+?)Z ') { " at " + $Matches[1].Substring(11) + "Z in the log" } else { "" }
            Note ("trace written: {0} frames to {1}{2} (read {3:N1} s after the ask)" -f $fr, $leaf, $logAt, ((Get-Date) - $script:traceAsked).TotalSeconds); $script:tracePending = $null; $script:traceSeen++ }
        elseif ($l -match 'trace not written(.*)$') { Note ("TRACE NOT WRITTEN" + $Matches[1]); $script:tracePending = $null }
    }
}
function Trace-Wait([double]$maxS = 30) {
    # stops mode: no other reader of the tail, so read it here.
    $end = (Get-Date).AddSeconds($maxS)
    while ($script:tracePending -and (Get-Date) -lt $end) { Trace-Lines (Read-NewLines); if ($script:tracePending) { if (-not (Pause-Checked 0.1)) { return $false } } }
    if ($script:tracePending) { Note "trace $($script:tracePending) not written after $maxS s"; return $false }
    return $true
}
function Trace-Start([string]$name, [int]$frames) {
    if ($script:tracePending) { Note "trace $name asked while $($script:tracePending) is still pending - skipped"; return }
    $path = ("$out\$name.json").Replace("\", "/")
    $script:traceAsked = Get-Date
    $r = Call "engine.trace" ('{"path": "' + $path + '", "frames": ' + $frames + '}')
    Note ("trace ${name} ($frames frames): " + $r)
    if ($r -match '"recording"') { $script:tracePending = "$name.json" }
}

switch ($Mode) {
{ $_ -in "route", "tour" } {
    # They run on their own and exit by themselves; mirror their log lines and the memory.
    if (Wait-Built 600) { Note ("after build: " + (Mem)) }
    $lastMem = Get-Date; $seen = 0; $pat = if ($Mode -eq "route") { 'showcase::route: route: (.+)' } else { 'showcase::tour: (.+)' }
    # $UpKm: space.bodies every frame (one call answers a frame) until the last altitude is shot; the
    # shot is asked for the first frame seen at or past each altitude.
    $ups = @($UpKm.Split(",") | Where-Object { $_ } | ForEach-Object { [double]$_ }); $upNext = 0; $lastLog = Get-Date; $prevAlt = $null
    if ($ups.Count -and -not $client) { Note "UpKm needs the bridge client (sample something)"; $ups = @() }
    while (-not $p.HasExited -and (Check-User) -and -not (Test-Hung) -and -not (Test-Mem)) {
        $polling = $upNext -lt $ups.Count
        if ($polling) {
            $alt = Anchor-Alt
            if ($alt -ne $null -and $alt -ge $ups[$upNext] * 1000) {
                $km = $ups[$upNext]; Shot ("up_{0}km_bridge" -f $km)
                $after = Anchor-Alt
                Note ("UP {0} km: altitude {1:N0} m in the frame before the ask, {2:N0} m the frame before {3:N0} m after the shot's answer" -f $km, $alt, $prevAlt, $after)
                $upNext++
                while ($upNext -lt $ups.Count -and $after -ge $ups[$upNext] * 1000) { Note ("UP {0} km: passed before it could be shot" -f $ups[$upNext]); $upNext++ }
            }
            $prevAlt = $alt
            if (((Get-Date) - $lastLog).TotalMilliseconds -lt 250) { continue }
        }
        $lastLog = Get-Date
        $lines = @(Select-String -Path "$out\stderr.log" -Pattern $pat -ErrorAction SilentlyContinue)
        while ($seen -lt $lines.Count) { $v = $lines[$seen].Matches[0].Groups[1].Value; Note ("log: " + $v.Substring(0, [math]::Min(160, $v.Length))); $seen++ }
        if (((Get-Date) - $lastMem).TotalSeconds -ge 10) { Note ("mem: " + (Mem)); $lastMem = Get-Date }
        if (((Get-Date) - $script:t0).TotalMinutes -gt $MaxMinutes) { Note "$MaxMinutes min timeout"; break }
        if (-not $polling) { Start-Sleep -Milliseconds 250 }
    }
}
"loops" {
    # One screenshot and one render + terrain stats snapshot per stop in the first round, 20 s
    # into its 25 s dwell: the first stop starts when the world is built, each next one after a
    # "loop" line.
    $keys = $Spec.Split(":")[0].Split(","); $rounds = if ($Spec.Contains(":")) { [int]$Spec.Split(":")[1] } else { 5 }
    if ($client -and (Wait-Built 400)) {
        Note ("after build: " + (Mem)); if ($h -ne [IntPtr]::Zero) { Note ("window: " + (Window-Info)) }
        $due = [datetime]::MaxValue; $next = 0; $seen = 0; $lastLine = Get-Date; $firstDue = $false; $shot = @{}; $haltAt = [datetime]::MaxValue
        $traceStopList = @($TraceStops.Split(",") | Where-Object { $_ }); $traceAtU = [datetime]::MaxValue; $traceStop = $null; $armAtU = [datetime]::MaxValue
        function LogTime([string]$l) {
            if ($l -match '^(20\d\d-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3})') { return [datetime]::ParseExact($Matches[1], "yyyy-MM-ddTHH:mm:ss.fff", [System.Globalization.CultureInfo]::InvariantCulture, [System.Globalization.DateTimeStyles]::AssumeUniversal -bor [System.Globalization.DateTimeStyles]::AdjustToUniversal) }
            return (Get-Date).ToUniversalTime()
        }
        while (-not $p.HasExited -and (Check-User) -and -not (Test-Hung) -and -not (Test-Mem)) {
            # The arrival trace: armed at its set time after the stop before's loop line (slept to within ~15 ms).
            $nowU = (Get-Date).ToUniversalTime()
            if ($nowU -ge $armAtU.AddSeconds(-0.3)) {
                $ms = ($armAtU - (Get-Date).ToUniversalTime()).TotalMilliseconds; if ($ms -gt 0) { Start-Sleep -Milliseconds ([int]$ms) }
                $armAtU = [datetime]::MaxValue; Trace-Start "trace_${ArrivalTrace}_arrival" $ArrivalFrames
            }
            if ($nowU -ge $traceAtU -and -not $script:tracePending) { $traceAtU = [datetime]::MaxValue; Trace-Start "trace_$traceStop" $TraceStopFrames }
            if (-not $firstDue -and (Test-Path "$out\frames.csv") -and @(Get-Content "$out\frames.csv" -ErrorAction SilentlyContinue).Count -ge 2) { $firstDue = $true; $due = (Get-Date).AddSeconds(19); $lastLine = Get-Date; Note "frames running" }
            if ($next -lt 2 * $keys.Count -and (Get-Date) -ge $due) {
                $k = $keys[$next % $keys.Count]; Save-Stats "render_$k"; Save-Stats "terrain_$k" "terrain."; Shot ("{0:00}-{1}" -f ($next % $keys.Count + 1), $k); $shot[$k] = $true; $due = [datetime]::MaxValue
                if ($FastGoAfter -eq $k -and $fasts.Count -and -not (Test-Path $go)) { New-Item $go -Force | Out-Null; Note "fast samplers go (after the $k picture)" }
            }
            if ((Get-Date) -ge $haltAt) { New-Item $halt -Force | Out-Null; Note "fast samplers halted"; $haltAt = [datetime]::MaxValue }
            $new = @(Read-NewLines)
            if ($script:tracePending) { Trace-Lines $new }
            foreach ($l in $new) {
                if ($l -notmatch 'showcase::loops: loop (\d+)/\d+ at (\w+):') { continue }
                $lr = $Matches[1]; $lk = $Matches[2]; $lu = LogTime $l
                Note ("loop line: $lr $lk (logged {0:HH:mm:ss.fff}Z)" -f $lu)
                if ($FastGoAfter -and $lk -eq $FastGoAfter -and (Test-Path $go) -and -not (Test-Path $halt) -and $haltAt -eq [datetime]::MaxValue) { $haltAt = (Get-Date).AddSeconds($FastHaltAfter) }
                $seen++; $next = $seen; $due = [datetime]::MaxValue
                if ($next -lt 2 * $keys.Count -and -not $shot[$keys[$next % $keys.Count]]) { $due = (Get-Date).AddSeconds(20) }; $lastLine = Get-Date
                if ($next -lt $keys.Count * $rounds) {
                    $arriving = $keys[$next % $keys.Count]
                    if ($traceStopList -contains $arriving -and $next -lt $keys.Count) { $traceStop = $arriving; $traceAtU = $lu.AddSeconds($TraceAfter) }
                    if ($ArrivalTrace -and $next + 1 -lt $keys.Count -and $keys[$next + 1] -eq $ArrivalTrace) { $armAtU = $lu.AddSeconds($ArrivalArm); Note ("arrival trace for $ArrivalTrace armed for {0:HH:mm:ss.fff}Z" -f $armAtU) }
                }
            }
            if ($seen -ge $keys.Count * $rounds) { Note "all rounds done"; Pause-Checked 2 | Out-Null; break }
            if (((Get-Date) - $lastLine).TotalSeconds -gt 90) { Note "HANG: no loop line for 90 s - stopping"; break }
            if (((Get-Date) - $script:t0).TotalMinutes -gt $MaxMinutes) { Note "$MaxMinutes min timeout"; break }
            Start-Sleep -Milliseconds 50
        }
        Note ("end: " + (Mem))
    }
}
"gi" {
    $built = $client -and (Wait-Built 400)
    if ($built) { Shot "first_frames"; Save-Stats "first_frames" "render.ev100" }
    if ($built -and (Pause-Checked 5)) {
        Note ("goto plaza: " + (Call "showcase.goto" '{"place": "plaza"}'))
        if (Pause-Checked $Settle) { Note ("WINDOW start ({0}-{1} s after the jump)" -f $Settle, ($Settle + 20)) }
        if (Pause-Checked 7) { Save-Stats "render_a" }
        if (Pause-Checked 12) { Save-Stats "render_b"; Note "WINDOW end" }
        if (Pause-Checked 1) { Shot "plaza" }
        Pause-Checked 3 | Out-Null
    }
}
"stops" {
    # Each "key:secs": jump there, stand still secs, then render and terrain stats and a picture.
    # "walk:secs": hold move forward (the fly camera at ground speed) for secs from where it is.
    if ($client -and (Wait-Built 400) -and (Pause-Checked 5)) {
        Note ("after build: " + (Mem))
        foreach ($s in $Spec.Split(",")) {
            $k, $secs = $s.Split(":"); $secs = [double]$secs
            if ($k -eq "walk") {
                Note ("WALK start: " + (Call "input.axis" '{"action": "move", "x": 0.0, "y": 1.0, "frames": 100000}'))
                $ok = Pause-Checked $secs
                Note ("WALK end: " + (Call "input.axis" '{"action": "move", "x": 0.0, "y": 0.0, "frames": 1}'))
                Save-Stats "render_walk"; Save-Stats "terrain_walk" "terrain."
                if (-not $ok) { break }
                continue
            }
            if ($k -like "arrive-*") {
                # "arrive-<place>:secs": engine.trace $ArriveTraceFrames frames, and the moment it is answered (that
                # frame's start; the recording starts at its end) showcase.goto <place>, which the bridge runs at the
                # next frame's start: the first recorded frame is the arrival frame. Then engine.stats
                # $ArriveStatsPrefix back to back on this connection, $ArriveStatsFrames answers (one a frame), each
                # with its wall time -> arrival_stats.jsonl. Then stand the rest of secs, stats and a picture.
                $place = $k.Substring(7)
                if ($script:tracePending -and -not (Trace-Wait 30)) { }
                $tpath = ("$out\trace_${place}_arrival.json").Replace("\", "/"); $script:traceAsked = Get-Date
                $r1 = Call "engine.trace" ('{"path": "' + $tpath + '", "frames": ' + $ArriveTraceFrames + '}')
                $r2 = Call "showcase.goto" ('{"place": "' + $place + '"}')
                $tGo = (Get-Date).ToUniversalTime()
                if ($r1 -match '"recording"') { $script:tracePending = "trace_${place}_arrival.json" }
                $lines = New-Object System.Collections.Generic.List[string]
                for ($i = 0; $i -lt $ArriveStatsFrames; $i++) {
                    $r = Call "engine.stats" ('{"prefix": "' + $ArriveStatsPrefix + '"}')
                    $lines.Add(("{0:o} {1}" -f (Get-Date).ToUniversalTime(), $r))
                }
                Set-Content "$out\arrival_stats.jsonl" $lines
                Note ("ARRIVE trace_${place}_arrival ($ArriveTraceFrames frames): " + $r1)
                Note ("AT ${place} (goto answered {0:HH:mm:ss.fff}Z): {1}" -f $tGo, $r2)
                Note ("ARRIVE: $ArriveStatsFrames engine.stats `"$ArriveStatsPrefix`" answers -> arrival_stats.jsonl, the last at {0:HH:mm:ss.fff}Z" -f (Get-Date).ToUniversalTime())
                $ok = Pause-Checked ($secs - ((Get-Date).ToUniversalTime() - $tGo).TotalSeconds)
                if (-not $ok) { break }
                if ($script:tracePending -and -not (Trace-Wait 30)) { }
                Save-Stats "render_$place"; Save-Stats "terrain_$place" "terrain."
                Note ("END ${place}: " + (Mem))
                Shot $place
                if (-not (Pause-Checked 1)) { break }
                continue
            }
            Note ("AT ${k}: " + (Call "showcase.goto" ('{"place": "' + $k + '"}')))
            $ok = $true
            if ($DumpEvery -gt 0) {
                $at = Get-Date; $n = 0; $traced = $TraceAt -lt 0
                while ($ok -and ((Get-Date) - $at).TotalSeconds -lt $secs - 0.05) {
                    $n++; $wait = [math]::Min($DumpEvery * $n, $secs) - ((Get-Date) - $at).TotalSeconds
                    if (-not $traced -and $TraceAt -lt $DumpEvery * $n) { $wait = $TraceAt - ((Get-Date) - $at).TotalSeconds; $n-- }
                    if ($wait -gt 0) { $ok = Pause-Checked $wait }
                    if (-not $ok) { break }
                    $sAt = ((Get-Date) - $at).TotalSeconds
                    if ($n -gt 0 -and [math]::Abs($sAt - $DumpEvery * $n) -lt 0.5) { Save-Stats ("dump_{0}_{1:000}s" -f $k, [math]::Round($sAt)) $DumpPrefix }
                    if (-not $traced -and $sAt -ge $TraceAt - 0.05) { $traced = $true; Trace-Start "trace_$k" $TraceFrames }
                    if ($script:tracePending) { Trace-Lines (Read-NewLines) }
                }
            } elseif ($TraceAt -ge 0) {
                # No dumps: stand to $TraceAt, trace, stand the rest.
                $at = Get-Date
                $ok = Pause-Checked $TraceAt
                if ($ok) { Trace-Start "trace_$k" $TraceFrames; $ok = Pause-Checked ($secs - ((Get-Date) - $at).TotalSeconds) }
            } else { $ok = Pause-Checked $secs }
            if (-not $ok) { break }
            if ($script:tracePending -and -not (Trace-Wait 30)) { }
            Save-Stats "render_$k"; Save-Stats "terrain_$k" "terrain."
            Note ("END ${k}: " + (Mem))
            Shot $k
            if (-not (Pause-Checked 1)) { break }
        }
        Pause-Checked 2 | Out-Null
    }
}
"night" {
    # For each hour in $Hours: set it with showcase.time (the clock stopped), then visit each $Spec place,
    # stand $NightSettle s, save space.bodies, weather.get, weather.panel and render.* there, and a picture.
    if ($client -and (Wait-Built 400) -and (Pause-Checked 5)) {
        Note ("after build: " + (Mem))
        Set-Content "$out\zones.json" (Call "showcase.zones"); Set-Content "$out\places.json" (Call "showcase.places")
        Set-Content "$out\weather_panel_start.json" (Call "weather.panel")
        $ok = $true
        foreach ($hs in $Hours.Split(",")) {
            Note ("TIME ${hs}: " + (Call "showcase.time" ('{"hour": ' + $hs + ', "hours_per_second": 0.0}')))
            foreach ($k in $Spec.Split(",")) {
                $tag = "h{0}_{1}" -f $hs.Replace(".", "_"), $k
                Note ("AT $k at ${hs} h: " + (Call "showcase.goto" ('{"place": "' + $k + '"}')))
                if (-not (Pause-Checked $NightSettle)) { $ok = $false; break }
                Set-Content "$out\bodies_$tag.json" (Call "space.bodies"); Set-Content "$out\weather_$tag.json" (Call "weather.panel")
                Save-Stats "render_$tag"
                Shot $tag
                Note ("END ${tag}: " + (Mem))
                if (-not (Pause-Checked 1)) { $ok = $false; break }
            }
            if (-not $ok) { break }
        }
        Pause-Checked 2 | Out-Null
    }
}
"mem" {
    # Jump to $Spec and hold $HoldMin minutes; every 30 s the process memory, the terrain's
    # patch counts and the entity count.
    if ($client -and (Wait-Built 400) -and (Pause-Checked 5)) {
        Note ("after build: " + (Mem))
        Note ("AT ${Spec}: " + (Call "showcase.goto" ('{"place": "' + $Spec + '"}')))
        $csv = "$out\mem.csv"; Set-Content $csv "secs_at_place,working_set_mb,private_mb,free_mb,entities,patches_building,patches_missing,patches_resident,patches_drawn,patches_built"
        $at = Get-Date; $n = 0
        while ($true) {
            $q = Get-Process -Id $p.Id -ErrorAction SilentlyContinue; $os = Get-CimInstance Win32_OperatingSystem
            $j = Call "engine.stats" '{"prefix": "terrain.patches"}' | ConvertFrom-Json
            $m = $j.result.metrics
            $row = "{0:F0},{1:F0},{2:F0},{3:F0},{4},{5},{6},{7},{8},{9}" -f ((Get-Date) - $at).TotalSeconds, ($q.WorkingSet64 / 1MB), ($q.PrivateMemorySize64 / 1MB), ($os.FreePhysicalMemory / 1KB), $j.result.entities,
                $m.'terrain.patches_building', $m.'terrain.patches_missing', $m.'terrain.patches_resident', $m.'terrain.patches_drawn', $m.'terrain.patches_built'
            Add-Content $csv $row; Note "mem: $row"
            if ($n -eq 1) { Shot $Spec }
            $n++
            if (((Get-Date) - $at).TotalMinutes -ge $HoldMin) { Note "held $HoldMin min"; break }
            $wait = 30 * $n - ((Get-Date) - $at).TotalSeconds
            if (-not (Pause-Checked ([math]::Max(1, $wait)))) { break }
        }
    }
}
}

if ($sampler) { New-Item $stop -Force | Out-Null; Wait-Job $sampler -Timeout 15 | Out-Null; Receive-Job $sampler *> "$out\sampler_job.txt"; Note ("sampler job: " + $sampler.State); Remove-Job $sampler -Force }
$k = 0; foreach ($fast in $fasts) { $k++; New-Item $stop -Force | Out-Null; Wait-Job $fast -Timeout 15 | Out-Null; Receive-Job $fast *> "$out\fast_job$k.txt"; Note ("fast sampler job ${k}: " + $fast.State); Remove-Job $fast -Force }
if ($client) { $client.Close() }
if (-not $p.HasExited) { Note "killing the game"; taskkill /IM showcase.exe /F | Out-Null } else { Note ("game exited by itself, code {0}" -f $p.ExitCode) }
Start-Sleep -Seconds 2
taskkill /IM showcase.exe /F 2>&1 | Out-Null
if (@(Get-Process showcase -ErrorAction SilentlyContinue).Count) { Note "showcase still running after taskkill!" }
# nvidia-smi writes its file in blocks: keep it logging (the idle GPU) a while so the run's
# last seconds reach the file before it is stopped.
if (-not $script:userBack) { Start-Sleep -Seconds 40 }
Stop-Process -Id $dmon.Id -Force -ErrorAction SilentlyContinue
Get-Process nvidia-smi -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
if (Test-Path "$repo\screenshots") { Move-Item "$repo\screenshots" "$out\png"; & "$sp\jpeg.ps1" -Src "$out\png" -Dst "$out\jpg" }
foreach ($f in "engine-profile.txt", "census.md", "census.details.md") { if (Test-Path "$repo\$f") { Move-Item "$repo\$f" "$out\$f" } }
if ($script:userBack) { Note "STOPPED: user came back" }
Note "done"
exit 0

