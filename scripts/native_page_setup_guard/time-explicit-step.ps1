function Invoke-PageSetupTimedStep {
    param(
        [Parameter(Mandatory)][ValidateSet('effect','capture','transfer','review','arm')][string]$Kind,
        [Parameter(Mandatory)][string]$ReceiptPath,
        [Parameter(Mandatory)][scriptblock]$Action
    )
    $ErrorActionPreference='Stop'
    # Reserve evidence before action: existing paths must refuse without effects.
    $stream=[IO.File]::Open($ReceiptPath,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    $clock=[Diagnostics.Stopwatch]::StartNew()
    $start=[Diagnostics.Stopwatch]::GetTimestamp()
    $receipt=[ordered]@{kind=$Kind;clock='local-stopwatch';frequency=[Diagnostics.Stopwatch]::Frequency;start_ticks=$start;end_ticks=$null;elapsed_ms=$null;outcome='exception';native_exit=$null}
    try {
        $global:LASTEXITCODE=$null
        & $Action
        $receipt.native_exit=$global:LASTEXITCODE
        if($null -ne $receipt.native_exit -and $receipt.native_exit -ne 0){throw 'Explicit step returned nonzero; retain uncertainty, no retry'}
        $receipt.outcome='returned'
    } finally {
        $receipt.end_ticks=[Diagnostics.Stopwatch]::GetTimestamp()
        $receipt.elapsed_ms=$clock.Elapsed.TotalMilliseconds
        $clock.Stop()
        # Exclusive creation preserves earlier evidence; never overwrite a receipt.
        try {$bytes=[Text.UTF8Encoding]::new($false).GetBytes(($receipt|ConvertTo-Json -Compress)+"`n");$stream.Write($bytes,0,$bytes.Length)}
        finally {$stream.Dispose()}
    }
}
