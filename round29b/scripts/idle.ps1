# Seconds since the last user keyboard/mouse input in this session (GetLastInputInfo).
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Idle {
    [StructLayout(LayoutKind.Sequential)] public struct LII { public uint cbSize; public uint dwTime; }
    [DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LII p);
    public static double Seconds() {
        var l = new LII(); l.cbSize = (uint)Marshal.SizeOf(typeof(LII));
        GetLastInputInfo(ref l);
        return ((uint)Environment.TickCount - l.dwTime) / 1000.0;
    }
}
"@ -ErrorAction SilentlyContinue
function Get-IdleSeconds { [Idle]::Seconds() }
# True when there has been user input since $Since (the input is newer than the run).
function Test-UserBack([datetime]$Since) { (Get-IdleSeconds) -lt (((Get-Date) - $Since).TotalSeconds - 2) }
