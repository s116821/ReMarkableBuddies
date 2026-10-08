$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-request-history-collector.ps1"
. "$PSScriptRoot/capture-observation-collector.ps1"
$nonce='0123456789abcdef0123456789abcdef';$root='/run/rmb-qt-probe-'+$nonce
$identity=[pscustomobject]@{attempt_pid='1234';attempt_start='5678';root_device='11';root_inode='22'}
$canonical="$nonce 1234 5678 11 22 capture-observation 120000 main-dev-facts-120s`n"
$count=0
function Check([bool]$ok,[string]$name){if(-not $ok){throw "FAIL $name"};$script:count++}
function Digest([byte[]]$value){[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($value)).ToLowerInvariant()}
$base=Join-Path ([IO.Path]::GetTempPath()) ('request-history-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $base)
try{
 foreach($case in @('refusal-no-png','both','tmp-only','absent','known-request','foreign-local','empty','malformed','partial','wrong-identity','different-tmp','read-timeout','copy-timeout','copy-truncated','later-loss','replaced','noisy')){
    $packet=Join-Path $base $case;[void](New-Item -ItemType Directory -Path $packet)
    $record=[ordered]@{capture_verified=$false;facts_verified=$false};$fixtureReads=@{};$copies=0
    $fixtureBytes=[Text.Encoding]::UTF8.GetBytes($(switch($case){'empty'{''};'malformed'{'broken'};'partial'{$canonical.Substring(0,20)};'wrong-identity'{$canonical.Replace('1234','9999')};default{$canonical}}))
    if($case -cin @('known-request','foreign-local')){
        [IO.File]::WriteAllText((Join-Path $packet 'capture-observation-request'),$canonical,[Text.UTF8Encoding]::new($false))
        if($case -ceq 'known-request'){$record.capture_request_saved_copy_verified=$true;$record.capture_request_saved_copy_sha256=Digest $fixtureBytes}
    }
    $read={param($command)
        Check ($command.Contains("11 22 700 0") -and $command.Contains("1234 5678") -and $command.Contains("/proc/1234") -and $command.Contains('current_start=')) 'original root attempt gone guards'
        $fixtureName=$(if($command.Contains('capture-observation-request.tmp')){'tmp'}else{'request'})
        $fixtureReads[$fixtureName]=1+$fixtureReads[$fixtureName]
        if($case -ceq 'read-timeout' -or ($case -ceq 'later-loss' -and $fixtureReads[$fixtureName] -gt 1)){return @{exit=-1;timeout=$true;stdout=''}}
        if($case -ceq 'absent' -or ($fixtureName -ceq 'tmp' -and $case -cnotin @('both','tmp-only','different-tmp')) -or ($fixtureName -ceq 'request' -and $case -ceq 'tmp-only')){return @{exit=0;timeout=$false;stdout="absent`n"}}
        $returned=$fixtureBytes;if($case -ceq 'different-tmp' -and $fixtureName -ceq 'tmp'){$returned=[Text.Encoding]::UTF8.GetBytes('wrong')}
        $inode=$(if($case -ceq 'replaced' -and $fixtureReads[$fixtureName] -gt 1){'100'}else{'99'})
        @{exit=0;timeout=$false;stdout=$(if($case -ceq 'noisy'){"active`n"}else{''})+"present $($returned.Length) $(Digest $returned) 11 $inode`n"}
    }
    $copy={param($remotePath,$localPath)$script:copies++
        if($case -ceq 'copy-timeout'){return @{exit=-1;timeout=$true}}
        $returned=$fixtureBytes;if($case -ceq 'different-tmp' -and $remotePath.EndsWith('.tmp')){$returned=[Text.Encoding]::UTF8.GetBytes('wrong')}elseif($case -ceq 'copy-truncated'){$returned=[byte[]]@(1,2)}
        [IO.File]::WriteAllBytes($localPath,$returned);@{exit=0;timeout=$false}
    }
    $threw=$false;try{Receive-CaptureRequestHistory $root $nonce $identity $packet $record $read $copy}catch{$threw=$true}
    $success=$case -cin @('refusal-no-png','both','tmp-only','absent','known-request')
    Check ($threw -ne $success) "refusal $case"
    Check (-not $record.capture_verified -and -not $record.facts_verified) "no admission $case"
    if($case -ceq 'known-request'){Check ($copies -eq 0) 'known exact copy reused'}
    if($case -ceq 'foreign-local'){Check ($copies -eq 0 -and [IO.File]::ReadAllText((Join-Path $packet 'capture-observation-request')) -ceq $canonical) 'foreign local never overwritten'}
    if($case -cin @('later-loss','replaced')){Check ($record.capture_request_saved_copy_verified -and -not $record.capture_request_history_binding_verified) "known copy survives binding loss $case"}
    if($case -cin @('empty','malformed','partial','wrong-identity')){Check ($record.capture_request_history_copy_verified -and -not $record.capture_request_saved_copy_verified) "noncanonical raw copy retained $case"}
    $preservation={param($command)
        $fixtureName=$(if($command.Contains('capture-observation-request.tmp')){'capture-observation-request.tmp'}elseif($command.Contains('capture-observation-request')){'capture-observation-request'}else{''})
        $fixturePath=Join-Path $packet $fixtureName
        if(-not $fixtureName -or -not(Test-Path -LiteralPath $fixturePath -PathType Leaf)){return @{exit=0;timeout=$false;stdout="absent`n"}}
        @{exit=0;timeout=$false;stdout="present $((Get-Item -LiteralPath $fixturePath).Length) $((Get-FileHash -LiteralPath $fixturePath).Hash.ToLowerInvariant())`n"}
    }
    if($success){Check (Test-CapturePreserved $root $packet $record $preservation) "refusal outputs closeout eligible $case"}
    elseif($case -cin @('later-loss','replaced','empty','malformed','partial','wrong-identity','foreign-local','different-tmp')){Check (-not(Test-CapturePreserved $root $packet $record $preservation)) "unknown current binding retains stage $case"}
 }
 Write-Output "PASS capture request history $count checks"
}finally{
 $resolved=[IO.Path]::GetFullPath($base);$prefix=[IO.Path]::GetFullPath([IO.Path]::GetTempPath())
 if(-not $resolved.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -cnotmatch '\Arequest-history-[0-9a-f]{32}\z'){throw 'Fixture cleanup path refused'}
 Remove-Item -LiteralPath $resolved -Recurse -Force
}
