param([double]$MaxHours = 6, [string]$Args27 = "")
# Wait until the user has had no input for 5+ min (with a few seconds' margin), then run run27.ps1 with
# $Args27 (a PowerShell argument string). Never starts while a build or the game is running.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\a9b51fd2-8307-4505-9047-44a5b14953cc\scratchpad\r27"
. "$sp\idle.ps1"
$t0 = Get-Date
while ($true) {
    $idle = Get-IdleSeconds
    $busy = @(Get-Process cargo, rustc, showcase -ErrorAction SilentlyContinue).Count
    if ($idle -ge 305 -and -not $busy) { break }
    if (((Get-Date) - $t0).TotalHours -gt $MaxHours) { "gave up after $MaxHours h (idle {0:N0} s)" -f $idle; exit 3 }
    Start-Sleep -Seconds ([math]::Max(5, [math]::Min(60, 305 - $idle)))
}
("idle {0:N0} s after waiting {1:N0} s; starting run27.ps1 " -f (Get-IdleSeconds), ((Get-Date) - $t0).TotalSeconds) + $Args27
Invoke-Expression "& `"$sp\run27.ps1`" $Args27"
