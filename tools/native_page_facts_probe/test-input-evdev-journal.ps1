param([string]$ChildMode)
$ErrorActionPreference='Stop'
if($ChildMode){
    $out=[Console]::OpenStandardOutput();$err=[Console]::OpenStandardError()
    switch($ChildMode){
        both {$err.Write([byte[]]::new(1024));$out.Write([byte[]]::new(65500))}
        stdout {$out.Write([byte[]]::new(80000))}
        stderr {$err.Write([byte[]]::new(80000))}
        timeout {Start-Sleep -Seconds 7}
        nonutf8 {$out.Write([byte[]]@(255,254,10))}
    }
    exit 0
}
. "$PSScriptRoot/input-evdev-journal.ps1"
$count=0
function Check([bool]$ok,[string]$label){if(-not $ok){throw "FAIL $label"};$script:count++}
$nonce='0123456789abcdef0123456789abcdef';$pidText='1234';$boot='0123456789abcdef0123456789abcdef'
$begin=Get-InputEvdevEndpoint "01234567-89ab-cdef-0123-456789abcdef 100 10.00`n"
$end=Get-InputEvdevEndpoint "01234567-89ab-cdef-0123-456789abcdef 101 11.00`n"
$window=Get-InputEvdevWindow $begin $end
Check ($window.boot -ceq $boot -and $window.begin_s -eq 100 -and $window.end_s -eq 102) 'fixed UTC enclosure'
foreach($text in @('',"01234567-89ab-cdef-0123-456789abcdef 100 10.00`r`n","01234567-89ab-cdef-0123-456789abcdef 0100 10.00`n",('x'*129))){Check ($null -eq (Get-InputEvdevEndpoint $text)) 'endpoint format refused'}
foreach($changed in @(@{boot='f'*32;realtime_s=101;boottime_cs=1100},@{boot=$boot;realtime_s=99;boottime_cs=1100},@{boot=$boot;realtime_s=101;boottime_cs=999},@{boot=$boot;realtime_s=250;boottime_cs=16000},@{boot=$boot;realtime_s=110;boottime_cs=1100})){
    Check ($null -eq (Get-InputEvdevWindow $begin $changed)) 'boot/clock/deadline refusal'
}
function Row([string]$message='qt.qpa.input.events: TouchPoint(1 @ area normalized point press 1 vel v state 1'){
    @{MESSAGE=$message;_PID=$pidText;_BOOT_ID=$boot;__REALTIME_TIMESTAMP='100500000';__MONOTONIC_TIMESTAMP='10500000'}
}
function Capture([string[]]$rows,[string]$trailer="QUERY_EXIT_$nonce 0`n"){
    @{exit=0;timeout=$false;acquisition_error=$false;stdout_overflow=$false;stderr_overflow=$false;stdout_bytes=[Text.Encoding]::UTF8.GetBytes(($rows -join "`n")+$(if($rows.Count){"`n"}else{''})+$trailer);stderr_bytes=[byte[]]@()}
}
function Decode($capture){Convert-InputEvdevJournal $capture $window $pidText $nonce}
$row=(Row|ConvertTo-Json -Compress)
$good=Decode (Capture @($row))
Check ($good.diagnostic_status -ceq 'captured' -and $good.point_records -eq 1) 'tagged point no closing token required'
foreach($key in @('MESSAGE','_PID','_BOOT_ID','__REALTIME_TIMESTAMP','__MONOTONIC_TIMESTAMP')){
    foreach($invalid in @($null,@('value'),1,$true)){
        $bad=Row;$bad[$key]=$invalid
        Check ((Decode (Capture @(($bad|ConvertTo-Json -Compress)))).diagnostic_status -ceq 'unknown') "scalar field required $key"
    }
    $bad=Row;$bad.Remove($key)
    Check ((Decode (Capture @(($bad|ConvertTo-Json -Compress)))).diagnostic_status -ceq 'unknown') "missing field $key"
}
foreach($pair in @(@('_PID','5678'),@('_BOOT_ID',('f'*32)),@('__REALTIME_TIMESTAMP','99500000'),@('__REALTIME_TIMESTAMP','103000000'),@('__REALTIME_TIMESTAMP',"100500000`n"))){
    $bad=Row;$bad[$pair[0]]=$pair[1];Check ((Decode (Capture @(($bad|ConvertTo-Json -Compress)))).diagnostic_status -ceq 'unknown') 'origin/time refused'
}
foreach($trailer in @('',"QUERY_EXIT_$nonce 7`n","QUERY_EXIT_other 0`n","QUERY_EXIT_$nonce 0","QUERY_EXIT_$nonce 0`nextra`n")){
    Check ((Decode (Capture @($row) $trailer)).diagnostic_status -ceq 'unknown') 'producer/final trailer refusal'
}
Check ((Decode (Capture @())).diagnostic_status -ceq 'unknown') 'empty unknown'
Check ((Decode (Capture @((Row 'qt.qpa.input: evdevtouch startup bounds'|ConvertTo-Json -Compress)))).diagnostic_status -ceq 'unknown') 'startup only inconclusive'
$untagged=Decode (Capture @((Row 'TouchPoint(untagged)'|ConvertTo-Json -Compress)))
Check ($untagged.category_unknown -and $untagged.records.Count -eq 0) 'untagged point category unknown'
$mixed=Decode (Capture @($row,(Row 'unrelated candidate body'|ConvertTo-Json -Compress)))
Check ($mixed.dropped_records -eq 1 -and $mixed.records.Count -eq 1) 'unrelated body discarded'
$mixed=Decode (Capture @($row,(Row 'qt.qpa.input: keyboard unrelated body'|ConvertTo-Json -Compress)))
Check ($mixed.diagnostic_status -ceq 'captured' -and $mixed.dropped_records -eq 1 -and $mixed.records.Count -eq 1) 'tagged unrelated body discarded'
foreach($value in @('-1','01','1.5',"10500000`n",'9223372036854775808')){
    $bad=Row;$bad.__MONOTONIC_TIMESTAMP=$value
    Check ((Decode (Capture @(($bad|ConvertTo-Json -Compress)))).diagnostic_status -ceq 'unknown') 'malformed journal monotonic refused'
}
foreach($value in @('0','10000000','11009999','900000000')){
    $boundary=Row;$boundary.__MONOTONIC_TIMESTAMP=$value
    Check ((Decode (Capture @(($boundary|ConvertTo-Json -Compress)))).diagnostic_status -ceq 'captured') 'valid journal monotonic not compared to BOOTTIME'
}
$suspendedBegin=@{boot=$boot;realtime_s=100;boottime_cs=86401000}
$suspendedEnd=@{boot=$boot;realtime_s=101;boottime_cs=86401100}
$suspendedWindow=Get-InputEvdevWindow $suspendedBegin $suspendedEnd
$afterSuspend=Convert-InputEvdevJournal (Capture @($row)) $suspendedWindow $pidText $nonce
Check ($afterSuspend.diagnostic_status -ceq 'captured' -and $afterSuspend.records[0].Contains('10500000')) 'substantial prior suspend offset retained without false rejection'
Check ($suspendedWindow.clock_domains.endpoint_uptime.Contains('CLOCK_BOOTTIME') -and $suspendedWindow.clock_domains.journal_monotonic.Contains('syntax/range only')) 'receipt clock domains explicit'
Check ($window.end_s -eq $end.realtime_s+1 -and (Get-InputEvdevJournalCommand $window $pidText $nonce).Contains('--until=@102')) 'UTC upper bound includes exactly one second'
foreach($message in @("qt.qpa.input.events: TouchPoint(é漢`nline`rvalue",('qt.qpa.input.events: TouchPoint('+('é'*8000)))){
    $decoded=Decode (Capture @((Row $message|ConvertTo-Json -Compress)))
    Check ($decoded.diagnostic_status -ceq 'captured' -and $decoded.records.Count -eq 1) 'multibyte/escaped multiline complete record'
}
foreach($n in @(256,257)){
    $decoded=Decode (Capture (@($row)*$n))
    Check (($n -eq 256 -and $decoded.diagnostic_status -ceq 'captured') -or ($n -eq 257 -and $decoded.diagnostic_status -ceq 'unknown' -and $decoded.truncated)) 'raw count cap'
}
$bad=Capture @($row);$bad.stdout_bytes=[byte[]]@(255,254,10)
Check ((Decode $bad).diagnostic_status -ceq 'unknown') 'invalid UTF8'
$bad=Capture @($row);$bad.stdout_bytes=[byte[]]::new(65537)
Check ((Decode $bad).truncated) 'byte sentinel'
$bad=Capture @($row);$bad.stderr_overflow=$true
Check ((Decode $bad).truncated) 'stderr cap unknown'
$bad=Capture @('{"MESSAGE":"x","MESSAGE":"x"}')
Check ((Decode $bad).diagnostic_status -ceq 'unknown') 'duplicate JSON refused'
$program=(Get-Process -Id $PID).Path
foreach($mode in @('both','stdout','stderr','nonutf8','timeout')){
    $actual=Invoke-InputEvdevCapture $program @('-NoProfile','-File',$PSCommandPath,'-ChildMode',$mode)
    Check ($actual.stdout_bytes.Length -le 65537 -and $actual.stderr_bytes.Length -le 1024) "$mode bounded acquisition"
    switch($mode){
        both {Check ($actual.exit -eq 0 -and -not $actual.timeout -and -not $actual.acquisition_error -and $actual.stdout_bytes.Length -eq 65500 -and $actual.stderr_bytes.Length -eq 1024) 'concurrent drain finite streams'}
        stdout {Check ($actual.stdout_overflow -and -not $actual.timeout) 'stdout finite overflow'}
        stderr {Check ($actual.stderr_overflow -and -not $actual.timeout) 'stderr finite overflow no deadlock'}
        nonutf8 {Check ($actual.stdout_bytes[0] -eq 255) 'raw bytes before strict decode'}
        timeout {Check ($actual.timeout -and $actual.elapsed_ms -ge 5000 -and $actual.elapsed_ms -lt 5500) 'single deadline without wait extension'}
    }
}
$source=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-input-observation-source.ps1'))
Check ($source.Contains('$developmentEvdevLogging=$false') -and $source.Contains("throw 'SOURCE ONLY:")) 'default off and source guard'
Check ($source.Contains((Get-FileHash (Join-Path $PSScriptRoot 'input-evdev-journal.ps1')).Hash.ToLowerInvariant())) 'fixed helper hash'
$launch=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'launch.sh'))
$stock=$launch.Substring($launch.IndexOf('stock() {'),$launch.IndexOf('if ! test -d')-$launch.IndexOf('stock() {'))
Check (-not $stock.Contains('QT_QPA') -and -not $stock.Contains('QT_LOGGING') -and $launch.Contains('development_evdev_logging=0')) 'stock fallback unchanged logging off'
Check ($launch.Contains("QT_QPA_EVDEV_DEBUG=1 QT_LOGGING_RULES='qt.qpa.input.events.debug=true' LD_PRELOAD=")) 'candidate exec-only assignments'
$command=Get-InputEvdevJournalCommand $window $pidText $nonce
Check ($command.Contains('set +e') -and $command.Contains('query_status=$?') -and $command.Contains('head -c 65537') -and -not $command.Contains('--follow')) 'fixed status wrapper/no follow'
Write-Output "input-evdev journal: PASS $count assertions (owned local child streams; no device)"
