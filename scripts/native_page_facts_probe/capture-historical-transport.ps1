# Local staging only; retained transport bytes never grant live admission.
function New-CaptureHistoricalTransportPath([string]$TemporaryRoot=[IO.Path]::GetTempPath()) {
    if(-not [IO.Path]::IsPathFullyQualified($TemporaryRoot)){throw 'Historical temporary root must be absolute'}
    $parent=[IO.Path]::GetFullPath($TemporaryRoot)
    $directory=Join-Path $parent ('rmbh-'+[Guid]::NewGuid().ToString('N'))
    $path=Join-Path $directory 'returned-bytes'
    if($path.Length -gt 240){throw 'Historical transport path exceeds local bound'}
    $parentItem=Get-Item -LiteralPath $parent -Force -ErrorAction Stop
    if(-not $parentItem.PSIsContainer -or ($parentItem.Attributes -band [IO.FileAttributes]::ReparsePoint)){throw 'Historical temporary root must be a non-link directory'}
    [void](New-Item -ItemType Directory -Path $directory -ErrorAction Stop)
    $created=Get-Item -LiteralPath $directory -Force -ErrorAction Stop
    if(-not $created.PSIsContainer -or ($created.Attributes -band [IO.FileAttributes]::ReparsePoint) -or (Test-Path -LiteralPath $path)){throw 'Historical transport directory refused'}
    return $path
}
