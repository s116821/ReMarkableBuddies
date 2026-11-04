# Local prelaunch check; caller freezes both files and invokes before any launch.
function Assert-ReadFactsReadinessDependency([string]$Directory) {
    $observer=Join-Path $Directory 'observe-readiness.sh'
    $wrapper=Join-Path $Directory 'wait-readiness.ps1'
    foreach($path in @($observer,$wrapper)){
        if(-not(Test-Path -LiteralPath $path -PathType Leaf) -or ((Get-Item -LiteralPath $path).Attributes -band [IO.FileAttributes]::ReparsePoint)){throw 'Readiness dependency file refused'}
    }
    $text=[IO.File]::ReadAllText($wrapper)
    $matches=[regex]::Matches($text,"-cne '([0-9a-f]{64})'\)\{throw 'Read-only observer changed'\}")
    if($matches.Count -ne 1 -or (Get-FileHash -LiteralPath $observer).Hash.ToLowerInvariant() -cne $matches[0].Groups[1].Value){throw 'Readiness observer dependency mismatch'}
}
