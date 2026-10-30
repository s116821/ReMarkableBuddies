$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'time-explicit-step.ps1')
$folder=Join-Path ([IO.Path]::GetTempPath()) ('setup-timing-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory $folder)
try {
    foreach($kind in @('effect','capture','transfer','review','arm')){
        $path=Join-Path $folder ($kind+'.json')
        Invoke-PageSetupTimedStep -Kind $kind -ReceiptPath $path -Action {'selected result'}|Out-Null
        $r=Get-Content $path -Raw|ConvertFrom-Json
        if($r.kind -cne $kind -or $r.outcome -cne 'returned' -or $r.end_ticks -lt $r.start_ticks -or $r.elapsed_ms -lt 0 -or $r.frequency -le 0){throw 'Monotonic successful receipt'}
    }
    foreach($mode in @('exception','native-exit','cmdlet-error')){
        $path=Join-Path $folder ($mode+'.json');$threw=$false
        try{Invoke-PageSetupTimedStep -Kind effect -ReceiptPath $path -Action {
            if($mode -ceq 'exception'){throw 'selected failure'}
            if($mode -ceq 'cmdlet-error'){Write-Error 'selected error'}
            $global:LASTEXITCODE=90
        }}catch{$threw=$true}
        $r=Get-Content $path -Raw|ConvertFrom-Json
        if(-not $threw -or $r.outcome -cne 'exception' -or $r.end_ticks -lt $r.start_ticks){throw 'Failure receipt'}
    }
    $script:ran=$false;$threw=$false
    try{Invoke-PageSetupTimedStep -Kind effect -ReceiptPath (Join-Path $folder 'effect.json') -Action {$script:ran=$true}}catch{$threw=$true}
    if(-not $threw -or $script:ran){throw 'Existing evidence must refuse before action'}
    'PASS 9 local monotonic timing cases; no transport/device'
}finally{Remove-Item -LiteralPath $folder -Recurse -Force}
