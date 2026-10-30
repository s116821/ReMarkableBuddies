$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'facts-proof.ps1')
. (Join-Path $PSScriptRoot 'development-budget.ps1')
$operator=Join-Path $PSScriptRoot 'run-qt-page-facts-one-shot.ps1'
$tokens=$null;$errors=$null
[void][Management.Automation.Language.Parser]::ParseFile($operator,[ref]$tokens,[ref]$errors)
if($errors.Count){throw ($errors|Out-String)}
$source=[IO.File]::ReadAllText($operator)
$begin=$source.IndexOf('            [IO.File]::WriteAllText((Join-Path $packet ''callback.json'')')
$end=$source.IndexOf('            break',$begin)
if($begin -lt 0 -or $end -lt 0){throw 'Live branch extraction failed'}
$branch=[scriptblock]::Create($source.Substring($begin,$end-$begin))
$nonce='b3e3ca0475a84432a9b328218395de0c'
$doc='00000000-0000-4000-8000-000000000001'
$order=@(2..7|ForEach-Object {'00000000-0000-4000-8000-'+$_.ToString('x12')})
$expected=[pscustomobject]@{document=$doc;order=$order}
$packet=Join-Path ([IO.Path]::GetTempPath()) ('facts-operator-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory $packet)
$script:cases=0
function New-Facts {
    [pscustomobject]@{kind='development-observed-facts';nonce=$nonce;attempt_pid='1234';attempt_start='5678';
        document_id=$doc;order=$order;current_index=0;current_page_id=$order[0];instance='1';begin_epoch='0';end_epoch='0';
        required_metadata_validated=$true;required_connections_installed=$true;begin_ms=0;end_ms=1;request_accepted_ms=119999;delivered_ms=120010;
        atomic_snapshot=$false;native_authority=$false;render_authority=$false;development_setup_opt_in=$true;setup_budget_ms=120000;setup_selection='main-dev-facts-120s'}
}
function Hash([string]$path){(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}
function Expand([string]$text){return $text}
function Require($result){if($result.timeout -or $result.exit -ne 0){throw 'mock refusal'}}
function ObservationSSH([string]$command){
    $script:transportCalls++
    if($script:failure -ceq 'budget'){throw 'budget exhausted'}
    if($script:failure -ceq 'transport'){return @{timeout=$true;exit=0;stdout=''}}
    if($script:transportCalls -eq 1){
        if($script:failure -ceq 'generation'){return @{timeout=$false;exit=90;stdout=''}}
        if($script:failure -ceq 'tuple'){return @{timeout=$false;exit=0;stdout="1234 5678 extra`n"}}
        return @{timeout=$false;exit=0;stdout="1234 5678 10 20`n"}
    }
    if($script:transportCalls -eq 2){return @{timeout=$false;exit=0;stdout=($script:diag|ConvertTo-Json -Depth 5 -Compress)}}
    return @{timeout=$false;exit=$(if($script:failure -ceq 'final-generation'){90}else{0});stdout=''}
}
try{
    foreach($mode in @('success','before-live-boundary','at-live-boundary','after-live-boundary','legacy','diagnostics','generation','final-generation','tuple','budget','transport')){
        $record=@{facts_verified=$false;candidate_generation_verified=$false}
        $script:transportCalls=0;$script:failure=$mode;$script:diag=New-Facts
        $observationClock=[pscustomobject]@{ElapsedMilliseconds=$(switch($mode){'before-live-boundary'{149999};'at-live-boundary'{150000};'after-live-boundary'{150001};default{500}})}
        $callback=[pscustomobject]@{nonce=$nonce;stage='facts-observed-no-change-during-read';application_thread=$true;engine_thread=$true}
        if($mode -ceq 'legacy'){$callback.stage='resolved'}
        if($mode -ceq 'diagnostics'){$script:diag.native_authority=$true}
        $observed=@{stdout=($callback|ConvertTo-Json -Compress)}
        $threw=$false;try{. $branch}catch{$threw=$true}
        if($record.facts_verified -ne ($mode -cin @('success','before-live-boundary'))){throw "Live proof result $mode"}
        if($threw -ne ($mode -cin @('at-live-boundary','after-live-boundary','generation','final-generation','tuple','budget','transport'))){throw "Live refusal $mode"}
        $script:cases++
    }
    # Exercise exact mandatory recovery, including historical-only collection.
    $recoveryStart=$source.IndexOf('}finally{')+10
    $recovery=[scriptblock]::Create($source.Substring($recoveryStart,$source.Length-$recoveryStart-2))
    function Start-Sleep {param($Seconds,$Milliseconds)}
    function Native {param($program,$arguments) return @{exit=0;timeout=$false;stdout=''}}
    function SSH([string]$command){
        $script:recoveryCommands+=$command
        if($command.Contains('stock-restored-and-exact-stage-removed')){
            $script:cleanupCalled=$true
            foreach($required in @('facts-publisher','check-waiting.sh','publish-facts-once.sh','facts-waiting','facts-request','facts-request.tmp')){
                if(-not $command.Contains($required)){throw "Cleanup omits $required"}
            }
            return @{exit=$(if($script:recoveryMode -ceq 'cleanup-failure'){90}else{0});timeout=$false;stdout=''}
        }
        if($command.Contains('stock-verified')){return @{exit=$(if($script:recoveryMode -ceq 'restore-failure'){90}else{0});timeout=$false;stdout=''}}
        if($command.Contains("printf 'present")){
            if($script:recoveryMode -ceq 'gate-failure' -and $command.Contains('facts-waiting')){return @{exit=90;timeout=$false;stdout=''}}
            return @{exit=0;timeout=$false;stdout="absent`n"}
        }
        return @{exit=0;timeout=$false;stdout=''}
    }
    foreach($mode in @('success','proof-refusal','restore-failure','gate-failure','cleanup-failure','historical-only')){
        $script:recoveryMode=$mode;$script:cleanupCalled=$false;$script:recoveryCommands=@()
        $record=@{arm_intent=$true;restored=$false;cleanup_verified=$false;callback_verified=($mode -cne 'historical-only');facts_verified=($mode -cin @('success','restore-failure','gate-failure','cleanup-failure'));candidate_generation_verified=($mode -cne 'historical-only');gate_evidence=@{}}
        $threw=$false;try{. $recovery|Out-Null}catch{$threw=$true}
        if($threw -ne ($mode -cne 'success')){throw "Recovery result $mode"}
        if($script:cleanupCalled -ne ($mode -cin @('success','proof-refusal','cleanup-failure','historical-only'))){throw "Recovery cleanup ordering $mode"}
        if(-not(Test-Path (Join-Path $packet 'operator-receipt.json'))){throw "Recovery receipt $mode"}
        if(-not($script:recoveryCommands[0].Contains('start --no-block'))){throw "Singleton restore trigger $mode"}
        if($mode -ceq 'historical-only' -and ($record.facts_verified -or $record.candidate_generation_verified)){throw 'Historical data upgraded live proof'}
        $script:cases++
    }
}finally{Remove-Item -LiteralPath $packet -Recurse -Force}
Write-Output "PASS $script:cases facts live/recovery branches; parser clean; mocked transport only"
