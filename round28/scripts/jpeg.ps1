param([string]$Src, [string]$Dst, [int]$Width = 1280, [long]$Quality = 85)
# Every PNG under $Src (a file or a folder) -> JPEG $Width wide in $Dst.
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force $Dst | Out-Null
$codec = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object { $_.MimeType -eq "image/jpeg" }
$ep = New-Object System.Drawing.Imaging.EncoderParameters 1
$ep.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter ([System.Drawing.Imaging.Encoder]::Quality), $Quality
foreach ($f in @(Get-ChildItem $Src -Recurse -Filter *.png)) {
    $img = [System.Drawing.Image]::FromFile($f.FullName)
    try {
        $w = [math]::Min($Width, $img.Width); $h = [int][math]::Round($img.Height * $w / $img.Width)
        $bmp = New-Object System.Drawing.Bitmap $w, $h
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.DrawImage($img, 0, 0, $w, $h); $g.Dispose()
        $bmp.Save((Join-Path $Dst ($f.BaseName + ".jpg")), $codec, $ep); $bmp.Dispose()
    } finally { $img.Dispose() }
}
