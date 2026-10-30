$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-request-history-collector.ps1"
. "$PSScriptRoot/capture-owner-refusal-collector.ps1"
. "$PSScriptRoot/capture-owner-refusal-proof.ps1"
. "$PSScriptRoot/capture-observation-collector.ps1"
$count=0
function Check([bool]$ok,[string]$name){if(-not $ok){throw "FAIL $name"};$script:count++}
function Digest([byte[]]$bytes){[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()}
$base=Join-Path ([IO.Path]::GetTempPath()) ('historical-test-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $base)
$paths=[Collections.Generic.List[string]]::new()
$nonce='0123456789abcdef0123456789abcdef';$remote='/run/rmb-qt-probe-'+$nonce
$attemptPid=[long]1234;$attemptStart='5678';$originalRootDevice='11';$originalRootInode='22'
$request=[Text.Encoding]::UTF8.GetBytes("$nonce 1234 5678 11 22 capture-observation 120000 main-dev-facts-120s`n")
$owner=[Text.Encoding]::UTF8.GetBytes(([ordered]@{kind='development-capture-owner-refusal';version=1;nonce=$nonce;attempt_pid='1234';attempt_start='5678';root_device='11';root_inode='22';setup_profile='main-dev-facts-120s';capture_accepted_ms=100;failure_ms=101;deadline_check_ms=$null;effective_deadline_ms=5100;branch='initial-progress';predicate='invalidated';discovery_result=$null;visited_items=$null;receiver_candidates=$null;scene_candidates=$null;matched_pairs=$null;first_pair_receiver=$null;first_pair_scene=$null;first_pair_rejection=$null;active_owner_rejection=$null;observer_role=$null;observer_member=$null;observer_failure=$null;native_authority=$false;render_authority=$false;ui_acknowledged=$false}|ConvertTo-Json -Compress))
$source=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-capture-observation-source.ps1'))
$begin=$source.IndexOf('            $ownerIdentity=');$end=$source.IndexOf('            $finalTransport=', $begin)
$historical=[scriptblock]::Create($source.Substring($begin,$end-$begin))
$begin=$source.IndexOf('            $visualPath=Join-Path');$end=$source.IndexOf('            $record.cleanup_verified=', $begin)
$cleanup=[scriptblock]::Create($source.Substring($begin,$end-$begin))
function Expand([string]$text){$text.Replace('@ROOT@',$remote)}
function Hash([string]$path){(Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant()}
try{
    foreach($bad in @('relative',(Join-Path $base 'missing'),($base+'/'+('x'*200)))){
        $refused=$false;try{$unused=New-CaptureHistoricalTransportPath $bad;$paths.Add($unused)}catch{$refused=$true}
        Check $refused 'invalid or overlong temporary root refused'
    }
    $link=Join-Path $base 'link'
    [void](New-Item -ItemType Junction -Path $link -Target $base)
    $refused=$false;try{$unused=New-CaptureHistoricalTransportPath $link;$paths.Add($unused)}catch{$refused=$true}
    Check $refused 'reparse temporary root refused'
    Remove-Item -LiteralPath $link -Force
    $deep=$base
    1..4|ForEach-Object {$deep=Join-Path $deep ('d'*60)}
    [void](New-Item -ItemType Directory -Path $deep)
    foreach($case in @('success','request-copy-failure','owner-copy-failure','both-copy-failure','request-later-loss','owner-later-loss')){
        $packet=Join-Path $deep $case;[void](New-Item -ItemType Directory -Path $packet)
        Check ((Join-Path $packet 'capture-observation-request').Length -gt 260) 'canonical packet reproduces deep path'
        $record=[ordered]@{capture_verified=$false;facts_verified=$false;restored=$true;cleanup_verified=$false}
        $reads=@{};$copies=@{};$cleanupCalls=0;$casePaths=[Collections.Generic.List[string]]::new()
        function SSH([string]$command){
            if($command.Contains('rmdir ')){$script:cleanupCalls++;return @{exit=0;timeout=$false;stdout=''}}
            $name=$(if($command.Contains('capture-owner-refusal.json')){'owner'}elseif($command.Contains('capture-observation-request.tmp')){'tmp'}elseif($command.Contains('capture-observation-request')){'request'}else{''})
            if(-not $name){return @{exit=0;timeout=$false;stdout="absent`n"}}
            $bytes=$(if($name -ceq 'owner'){$owner}else{$request});$sha=Digest $bytes
            if($command.Contains('11 22 700 0')){
                Check ([regex]::Matches($command,'--verify >/dev/null').Count -eq 2 -and $command.Contains('11 22 700 0') -and $command.Contains('1234 5678')) 'actual restored original guards around metadata'
                $reads[$name]=1+$reads[$name]
                if((($case -ceq 'request-later-loss' -and $name -ceq 'request') -or ($case -ceq 'owner-later-loss' -and $name -ceq 'owner')) -and $reads[$name] -gt 1){return @{exit=1;timeout=$false;stdout=''}}
                return @{exit=0;timeout=$false;stdout="present $($bytes.Length) $sha 11 99`n"}
            }
            return @{exit=0;timeout=$false;stdout="present $($bytes.Length) $sha`n"}
        }
        function Native([string]$program,[string[]]$arguments){
            Check ($program -ceq 'scp' -and $arguments[-2].StartsWith('RM2:'+ $remote+'/')) 'fixed bounded copy transport'
            $local=$arguments[-1];$name=$(if($arguments[-2].EndsWith('capture-owner-refusal.json')){'owner'}elseif($arguments[-2].EndsWith('.tmp')){'tmp'}else{'request'})
            $copies[$name]=1+$copies[$name];$paths.Add($local);$casePaths.Add($local)
            $key=$(if($name -ceq 'owner'){'capture_owner_refusal_transport_path'}elseif($name -ceq 'tmp'){'capture_request_tmp_history_transport_path'}else{'capture_request_history_transport_path'})
            Check ($record[$key] -ceq $local -and [IO.Path]::IsPathFullyQualified($local) -and $local.Length -le 240 -and -not(Test-Path -LiteralPath $local)) 'exclusive short path recorded before dispatch'
            if(($case -ceq 'both-copy-failure') -or ($case -ceq 'request-copy-failure' -and $name -ceq 'request') -or ($case -ceq 'owner-copy-failure' -and $name -ceq 'owner')){return @{exit=1;timeout=$false;stdout='';stderr='mock local open failure'}}
            [IO.File]::WriteAllBytes($local,$(if($name -ceq 'owner'){$owner}else{$request}))
            return @{exit=0;timeout=$false;stdout='';stderr=''}
        }
        $refused=$false;try{. $historical;. $cleanup}catch{$refused=$true}
        $requestGood=$case -cnotin @('request-copy-failure','both-copy-failure','request-later-loss')
        $ownerGood=$case -cnotin @('owner-copy-failure','both-copy-failure','owner-later-loss')
        Check ($copies.request -eq 1 -and $copies.owner -eq 1 -and [int]$copies.tmp -eq $(if($requestGood){1}else{0})) "independent collectors dispatched once without retry $case copies=$($copies|ConvertTo-Json -Compress) errors=$($record.historical_collection_errors|ConvertTo-Json -Compress)"
        Check ($refused -eq ($case -cne 'success') -and $cleanupCalls -eq $(if($case -ceq 'success'){1}else{0}) -and -not $record.cleanup_verified) 'any collector error blocks cleanup'
        Check ($record.historical_collection_errors.Count -eq $(if($case -ceq 'success'){0}elseif($case -ceq 'both-copy-failure'){2}else{1})) 'individual errors recorded'
        Check ([bool]$record.capture_request_saved_copy_verified -eq ($requestGood -or $case -ceq 'request-later-loss') -and [bool]$record.capture_owner_refusal_saved_copy_verified -eq ($ownerGood -or $case -ceq 'owner-later-loss')) 'successful other or later-lost complete copies remain known'
        Check (-not $record.capture_verified -and -not $record.facts_verified) 'historical preservation never grants admission'
        Check (@($casePaths|Select-Object -Unique).Count -eq $casePaths.Count) 'request tmp owner transport paths distinct'
        foreach($path in $casePaths){if(Test-Path -LiteralPath $path){Check ((Get-Item -LiteralPath $path).Length -gt 0) 'returned transport bytes retained'}}
    }
    Write-Output "PASS historical transport/isolation $count checks (actual source; mocked transport)"
}finally{
    foreach($path in $paths){
        $directory=[IO.Path]::GetFullPath((Split-Path $path))
        $temporary=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\','/')+[IO.Path]::DirectorySeparatorChar
        if(-not $directory.StartsWith($temporary,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($directory) -cnotmatch '\Armbh-[0-9a-f]{32}\z'){throw 'Transport fixture cleanup path refused'}
        if(Test-Path -LiteralPath $directory){Remove-Item -LiteralPath $directory -Recurse -Force}
    }
    $resolved=[IO.Path]::GetFullPath($base)
    if(-not $resolved.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()),[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -cnotmatch '\Ahistorical-test-[0-9a-f]{32}\z'){throw 'Historical fixture cleanup path refused'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
