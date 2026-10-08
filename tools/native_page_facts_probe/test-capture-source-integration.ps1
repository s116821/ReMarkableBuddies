$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-observation-build-config.ps1"
. "$PSScriptRoot/capture-observation-collector.ps1"
$source=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-capture-observation-source.ps1'))
$tokens=$null;$errors=$null;$ast=[Management.Automation.Language.Parser]::ParseInput($source,[ref]$tokens,[ref]$errors)
if($errors.Count){throw ($errors|Out-String)}
$functions=$ast.FindAll({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst]},$true)
. ([scriptblock]::Create(($functions|Where-Object Name -ceq 'ObservationCopy').Extent.Text))
$count=0
function Check([bool]$ok,[string]$name){if(-not $ok){throw "FAIL $name"};$script:count++}
$nonce='0123456789abcdef0123456789abcdef'
$expected=[pscustomobject]@{nonce=$nonce;document='11111111-1111-1111-1111-111111111111';order=@(2..7|ForEach-Object {'00000000-0000-4000-8000-'+$_.ToString('x12')})}
$config=Get-CaptureObservationBuildConfig $expected
Check ($config.Contains('config.developmentCaptureObservation=true;') -and $config.Contains('config.developmentInputObservation=false;')) 'explicit incompatible option binding'
foreach($bad in @('bad-nonce','bad-document','duplicate','five-pages')){
    $copy=$expected|ConvertTo-Json|ConvertFrom-Json
    switch($bad){'bad-nonce'{$copy.nonce='spent'};'bad-document'{$copy.document='bad'};'duplicate'{$copy.order[1]=$copy.order[0]};'five-pages'{$copy.order=$copy.order[0..4]}}
    $refused=$false;try{[void](Get-CaptureObservationBuildConfig $copy)}catch{$refused=$true};Check $refused "$bad build refusal"
}
$budget=[pscustomobject]@{LiveObservationMs=150000};$observationClock=[pscustomobject]@{ElapsedMilliseconds=149999};$nativeCalls=0;$bound=0
function Native([string]$program,[string[]]$arguments,[int]$timeoutMs){$script:nativeCalls++;$script:bound=$timeoutMs;$observationClock.ElapsedMilliseconds=150001;return @{exit=0;timeout=$false;stdout=''}}
$returned=ObservationCopy '/run/fixed/capture-window.png' '/not-written-by-mock'
Check ($returned.exit -eq 0 -and $nativeCalls -eq 1 -and $bound -eq 1) 'late returned copy available for preservation before live refusal'
$refused=$false;try{[void](ObservationCopy '/run/fixed/capture-window.png' '/not-written-by-mock')}catch{$refused=$true}
Check ($refused -and $nativeCalls -eq 1) 'expired copy does not dispatch'
$begin=$source.IndexOf('            $visualPath=Join-Path');$end=$source.IndexOf('            $record.cleanup_verified=', $begin)
$cleanup=[scriptblock]::Create($source.Substring($begin,$end-$begin))
$packet=Join-Path ([IO.Path]::GetTempPath()) ('capture-source-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $packet)
$remote='/run/rmb-qt-probe-'+$nonce
function Expand([string]$text){$text.Replace('@ROOT@',$remote)}
function Hash([string]$path){(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}
try{
    foreach($case in @('absent','unknown-image','unknown-json','unknown-owner')){
        $record=[ordered]@{restored=$true;cleanup_verified=$false};$cleanupCalls=0
        function SSH([string]$command){
            if($command.Contains('rmdir ')){$script:cleanupCalls++;return @{exit=0;timeout=$false;stdout=''}}
            if(($case -ceq 'unknown-owner' -and $command.Contains('capture-owner-refusal.json')) -or ($case -ceq 'unknown-image' -and $command.Contains('capture-window.png')) -or ($case -ceq 'unknown-json' -and $command.Contains('capture-observation-complete.json'))){return @{exit=1;timeout=$true;stdout=''}}
            return @{exit=0;timeout=$false;stdout="absent`n"}
        }
        $refused=$false;try{. $cleanup}catch{$refused=$true}
        Check ($cleanupCalls -eq $(if($case -ceq 'absent'){1}else{0}) -and $refused -eq ($case -cne 'absent')) "$case source cleanup dispatch"
    }
    $begin=$source.IndexOf('            $record.facts_verified=');$end=$source.IndexOf('            break',$begin)
    $admit=[scriptblock]::Create($source.Substring($begin,$end-$begin))
    function Test-FactsDiagnostics {return $true} # Isolate the added agreement gate.
    foreach($case in @('good','capture-false','wrong-document','wrong-page','wrong-index')){
        $record=[ordered]@{callback_verified=$true;capture_verified=($case -cne 'capture-false');capture_completion=[pscustomobject]@{document_id=$expected.document;page_id=$expected.order[0];page_index=0}}
        $diagnostics=[pscustomobject]@{document_id=$expected.document;current_page_id=$expected.order[0];current_index=0}
        switch($case){'wrong-document'{$diagnostics.document_id=$expected.order[0]};'wrong-page'{$diagnostics.current_page_id=$expected.order[1]};'wrong-index'{$diagnostics.current_index=1}}
        . $admit;Check ($record.facts_verified -eq ($case -ceq 'good')) "$case final facts agreement"
    }
    # Execute the actual historical collection block with the real decoder and
    # collector. Transport is mocked; no target operation is dispatched.
    . "$PSScriptRoot/capture-owner-refusal-proof.ps1"
    . "$PSScriptRoot/capture-owner-refusal-collector.ps1"
    $begin=$source.IndexOf('            $ownerIdentity=');$end=$source.IndexOf('            $finalTransport=', $begin)
    $historical=[scriptblock]::Create($source.Substring($begin,$end-$begin))
    $attemptPid=[long]1234;$attemptStart='5678';$originalRootDevice='11';$originalRootInode='22'
    $record=[ordered]@{capture_verified=$false;facts_verified=$false};$historicalReads=0
    function SSH([string]$command){
        $script:historicalReads++
        Check ($command.Contains("/bin/sh '$remote/restore.sh' --verify >/dev/null") -and $command.Contains("11 22 700 0") -and $command.Contains("1234 5678")) 'actual historical restored original guards'
        return @{exit=0;timeout=$false;stdout="absent`n"}
    }
    . $historical
    Check ($historicalReads -eq 1 -and $record.capture_owner_refusal_state -ceq 'absent' -and -not $record.capture_verified -and -not $record.facts_verified) 'actual historical absent collection independent of completion'
    Write-Output "PASS capture source integration: $count checks (mocked source path only)"
}finally{
    $resolved=[IO.Path]::GetFullPath($packet);$prefix=[IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if(-not $resolved.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -cnotmatch '\Acapture-source-[0-9a-f]{32}\z'){throw 'Fixture cleanup path refused'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
