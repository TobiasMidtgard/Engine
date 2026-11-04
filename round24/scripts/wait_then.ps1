param([double]$MaxHours = 6, [string]$Args24 = "")
# Wait until the user has had no input for 5+ min (with a few seconds' margin), then run run24.ps1 with
# $Args24 (a PowerShell argument string). Never starts while a build or the game is running.
$sp = "C:\Users\Tobit\AppData\Local\Temp\claude\C--Users-Tobit-Documents-ProgrammingProjects\45d985d3-0d66-4e84-94ac-e37b77e235f5\scratchpad\r24"
. "$sp\idle.ps1"
$t0 = Get-Date
while ($true) {
    $idle = Get-IdleSeconds
    $busy = @(Get-Process cargo, rustc, showcase -ErrorAction SilentlyContinue).Count
    if ($idle -ge 305 -and -not $busy) { break }
    if (((Get-Date) - $t0).TotalHours -gt $MaxHours) { "gave up after $MaxHours h (idle {0:N0} s)" -f $idle; exit 3 }
    Start-Sleep -Seconds ([math]::Max(5, [math]::Min(60, 305 - $idle)))
}
("idle {0:N0} s after waiting {1:N0} s; starting run24.ps1 " -f (Get-IdleSeconds), ((Get-Date) - $t0).TotalSeconds) + $Args24
Invoke-Expression "& `"$sp\run24.ps1`" $Args24"
