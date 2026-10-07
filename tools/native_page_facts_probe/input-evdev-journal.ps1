# Fixed development diagnostic only. No authority or recovery flag writes.
$InputEvdevSnapshotCommand=@'
printf '%s %s %s\n' "$(cat /proc/sys/kernel/random/boot_id)" "$(date -u +%s)" "$(cut -d' ' -f1 /proc/uptime)"
'@
function Get-InputEvdevEndpoint([string]$text) {
    if($text.Length -gt 128 -or $text -cnotmatch '\A([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}) ([1-9][0-9]{0,9}) (0|[1-9][0-9]{0,9})\.([0-9]{2})\n\z'){return $null}
    return @{boot=$Matches[1].Replace('-','');realtime_s=[long]$Matches[2];monotonic_cs=([long]$Matches[3]*100+[int]$Matches[4])}
}
function Get-InputEvdevWindow($begin,$end) {
    foreach($point in @($begin,$end)){
        if($null -eq $point -or $point.boot -isnot [string] -or $point.boot -cnotmatch '\A[0-9a-f]{32}\z'){return $null}
        foreach($key in @('realtime_s','monotonic_cs')){if(($point.$key -isnot [long] -and $point.$key -isnot [int]) -or $point.$key -lt 0){return $null}}
        if($point.realtime_s -lt 1 -or $point.realtime_s -gt 9999999999 -or $point.monotonic_cs -gt 999999999999){return $null}
    }
    if($null -eq $begin -or $null -eq $end -or $begin.boot -cne $end.boot){return $null}
    $wall=$end.realtime_s-$begin.realtime_s;$mono=$end.monotonic_cs-$begin.monotonic_cs
    if($wall -lt 0 -or $mono -lt 0 -or $wall+1 -gt 150 -or $mono -gt 15000 -or [Math]::Abs($wall*1000-$mono*10) -gt 1010){return $null}
    return @{boot=$begin.boot;begin_s=$begin.realtime_s;end_s=$end.realtime_s+1;begin_cs=$begin.monotonic_cs;end_cs=$end.monotonic_cs}
}
function Get-InputEvdevJournalCommand($window,[string]$pidText,[string]$nonce) {
    if($null -eq $window -or $nonce -cnotmatch '\A[0-9a-f]{32}\z' -or $pidText -cnotmatch '\A[1-9][0-9]{0,9}\z' -or [long]$pidText -gt 2147483647){throw 'Fixed journal metadata refused'}
    if($window.boot -isnot [string] -or $window.boot -cnotmatch '\A[0-9a-f]{32}\z'){throw 'Fixed journal boot refused'}
    foreach($key in @('begin_s','end_s')){if(($window.$key -isnot [long] -and $window.$key -isnot [int]) -or $window.$key -lt 1 -or $window.$key -gt 10000000000){throw 'Fixed journal clock refused'}}
    if($window.end_s -le $window.begin_s -or $window.end_s-$window.begin_s -gt 150){throw 'Fixed journal interval refused'}
    # No remote file. Explicit status survives caller errexit; head exit is irrelevant.
    return @"
(set +e
LC_ALL=C TZ=UTC journalctl --quiet --no-pager --all --utc --output=json --output-fields=MESSAGE,_PID,_BOOT_ID,__REALTIME_TIMESTAMP,__MONOTONIC_TIMESTAMP --lines=257 --since=@$($window.begin_s) --until=@$($window.end_s) _BOOT_ID=$($window.boot) _PID=$pidText
query_status=`$?
printf 'QUERY_EXIT_$nonce %s\n' "`$query_status"
) | head -c 65537
"@
}
function Invoke-InputEvdevCapture([string]$program,[string[]]$arguments) {
    $result=@{exit=$null;timeout=$false;stdout_overflow=$false;stderr_overflow=$false;acquisition_error=$false;stdout_bytes=[byte[]]@();stderr_bytes=[byte[]]@()}
    $clock=[Diagnostics.Stopwatch]::StartNew();$process=$null;$streams=@();$stores=@()
    try{
        $info=[Diagnostics.ProcessStartInfo]::new();$info.FileName=$program;$info.UseShellExecute=$false
        $info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
        foreach($arg in $arguments){$info.ArgumentList.Add($arg)}
        $process=[Diagnostics.Process]::Start($info)
        $streams=@($process.StandardOutput.BaseStream,$process.StandardError.BaseStream)
        $stores=@([IO.MemoryStream]::new(),[IO.MemoryStream]::new());$caps=@(65537,1024)
        $buffers=@([byte[]]::new(2048),[byte[]]::new(2048));$done=@($false,$false);$pending=@($null,$null)
        while($true){
            if($clock.ElapsedMilliseconds -ge 5000){$result.timeout=$true;break}
            for($i=0;$i -lt 2;$i++){
                if($done[$i]){continue}
                if($null -eq $pending[$i]){$pending[$i]=$streams[$i].ReadAsync($buffers[$i],0,$buffers[$i].Length)}
                if($pending[$i].IsCompleted){
                    $read=$pending[$i].GetAwaiter().GetResult();$pending[$i]=$null
                    if($read -eq 0){$done[$i]=$true;continue}
                    $room=$caps[$i]-$stores[$i].Length
                    $stores[$i].Write($buffers[$i],0,[int][Math]::Min($room,$read))
                    if($read -gt $room -or ($i -eq 0 -and $stores[$i].Length -eq 65537)){
                        if($i -eq 0){$result.stdout_overflow=$true}else{$result.stderr_overflow=$true}
                    }
                }
            }
            if($result.stdout_overflow -or $result.stderr_overflow){break}
            if($done[0] -and $done[1] -and $process.HasExited){$result.exit=$process.ExitCode;break}
            [Threading.Thread]::Sleep(1)
        }
    }catch{$result.acquisition_error=$true}
    finally{
        # Local child termination only; no guarantee about remote journal cancellation.
        if($process){try{if(-not $process.HasExited){$process.Kill($true)}}catch{$result.acquisition_error=$true}}
        for($i=0;$i -lt $stores.Count;$i++){
            if($i -eq 0){$result.stdout_bytes=$stores[$i].ToArray()}else{$result.stderr_bytes=$stores[$i].ToArray()}
            $stores[$i].Dispose()
        }
        foreach($stream in $streams){$stream.Dispose()}
        if($process){$process.Dispose()}
        $clock.Stop();$result.elapsed_ms=$clock.ElapsedMilliseconds
    }
    return $result
}
function Convert-InputEvdevJournal($capture,$window,[string]$pidText,[string]$nonce) {
    $result=@{diagnostic_status='unknown';truncated=$false;category_unknown=$false;raw_records=0;eligible_records=0;point_records=0;dropped_records=0;retained_bytes=0;records=@();query_exit=$null;reason='transport-or-endpoints'}
    if($null -eq $window -or $capture.timeout -or $capture.acquisition_error -or $capture.exit -ne 0){return $result}
    if($capture.stdout_overflow -or $capture.stderr_overflow -or $capture.stdout_bytes.Length -ge 65537){$result.truncated=$true;return $result}
    try{
        $utf8=[Text.UTF8Encoding]::new($false,$true);$text=$utf8.GetString($capture.stdout_bytes)
        if(-not $text.EndsWith("`n")){throw 'Missing final complete trailer'}
        $lines=$text.Substring(0,$text.Length-1).Split("`n")
        if($lines[-1] -cnotmatch ('\AQUERY_EXIT_'+[regex]::Escape($nonce)+' ([0-9]{1,3})\z')){throw 'Wrong final trailer'}
        $result.query_exit=[int]$Matches[1];if($result.query_exit -ne 0 -or $capture.stderr_bytes.Length -ne 0){throw 'Producer error'}
        $result.raw_records=$lines.Length-1
        if($result.raw_records -ge 257){$result.truncated=$true;throw 'Raw record sentinel'}
        for($i=0;$i -lt $result.raw_records;$i++){
            $doc=[Text.Json.JsonDocument]::Parse($lines[$i]);try{
                $root=$doc.RootElement;if($root.ValueKind -ne [Text.Json.JsonValueKind]::Object){throw 'Not an object'}
                $names=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
                foreach($property in $root.EnumerateObject()){if(-not $names.Add($property.Name)){throw 'Duplicate JSON key'}}
                $fields=@{}
                foreach($key in @('MESSAGE','_PID','_BOOT_ID','__REALTIME_TIMESTAMP','__MONOTONIC_TIMESTAMP')){
                    $value=$root.GetProperty($key);if($value.ValueKind -ne [Text.Json.JsonValueKind]::String){throw 'Non-scalar journal field'}
                    $fields[$key]=$value.GetString()
                }
                if($fields._PID -cne $pidText -or $fields._BOOT_ID -cne $window.boot){throw 'Wrong journal origin'}
                foreach($key in @('__REALTIME_TIMESTAMP','__MONOTONIC_TIMESTAMP')){
                    if($fields[$key] -cnotmatch '\A(0|[1-9][0-9]{0,18})\z'){throw 'Clock syntax'}
                    $parsed=[long]0;if(-not [long]::TryParse($fields[$key],[ref]$parsed)){throw 'Clock range'}
                }
                $us=[long]$fields.__REALTIME_TIMESTAMP
                if($us -lt $window.begin_s*1000000 -or $us -gt $window.end_s*1000000){throw 'Outside clock enclosure'}
                # Uptime endpoints have centisecond floor precision. A different
                # journal monotonic origin (for example after suspend) is unknown.
                $monoUs=[long]$fields.__MONOTONIC_TIMESTAMP
                if($monoUs -lt $window.begin_cs*10000 -or $monoUs -gt ($window.end_cs+1)*10000-1){throw 'Outside monotonic enclosure'}
                $tagged=$fields.MESSAGE -cmatch '\Aqt\.qpa\.input(?:\.events)?: '
                $inputDiagnostic=$fields.MESSAGE.Contains('TouchPoint(') -or $fields.MESSAGE.Contains('evdevtouch') -or $fields.MESSAGE.Contains('pressure')
                if($tagged -and $inputDiagnostic){
                    $bytes=$utf8.GetByteCount($lines[$i])+1
                    if($result.records.Count -eq 256 -or $result.retained_bytes+$bytes -gt 65536){$result.truncated=$true;throw 'Retained cap'}
                    $result.records+=($lines[$i]+"`n");$result.retained_bytes+=$bytes;$result.eligible_records++
                    if($fields.MESSAGE.StartsWith('qt.qpa.input.events: ') -and $fields.MESSAGE.Contains('TouchPoint(')){$result.point_records++}
                }else{
                    $result.dropped_records++
                    if(-not $tagged -and ($fields.MESSAGE.Contains('TouchPoint') -or $inputDiagnostic)){$result.category_unknown=$true}
                }
            }finally{$doc.Dispose()}
        }
        $result.reason='no-eligible-point-evidence'
        if($result.point_records -gt 0 -and -not $result.category_unknown){$result.diagnostic_status='captured';$result.reason='bounded-tagged-point-records; no completeness or formatter qualification'}
    }catch{$result.reason=$_.Exception.Message}
    return $result
}
