$ErrorActionPreference='Stop'
. "$PSScriptRoot/facts-refusal-proof.ps1"
. "$PSScriptRoot/facts-proof.ps1"
. "$PSScriptRoot/development-budget.ps1"
$source=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-page-facts-diagnostic-source.ps1'))
$t=$null;$e=$null;[void][Management.Automation.Language.Parser]::ParseInput($source,[ref]$t,[ref]$e)
if($e.Count){throw ($e|Out-String)}
$begin=$source.IndexOf('            [IO.File]::WriteAllText((Join-Path $packet ''callback.json'')')
$end=$source.IndexOf('            break',$begin)
$branch=[scriptblock]::Create($source.Substring($begin,$end-$begin))
$nonce='b3e3ca0475a84432a9b328218395de0c'
$order=@(2..7|ForEach-Object {'00000000-0000-4000-8000-{0:x12}' -f $_})
$expected=[pscustomobject]@{document='00000000-0000-4000-8000-000000000001';order=$order}
$packet=Join-Path ([IO.Path]::GetTempPath()) ('diagnostic-operator-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory $packet)
$count=0
function Check([bool]$ok,[string]$name){if(-not $ok){throw "FAIL $name"};$script:count++}
function New-Refusal {
    [pscustomobject]@{kind='development-facts-refusal';version=1;nonce=$nonce;attempt_pid='1234';attempt_start='5678';
      reader_stage='facts-values-refused';reader_result_had_facts=$false;entry_stage='facts-entry-read-refused';refusal_path='reader-result';
      sample_phase='before-refusal-callback';sample_ms=120010;request_accepted_ms=119999;accepted_read_elapsed_ms=11;
      setup_clock_origin='entry-startup-monotonic';read_clock_origin='accepted-request-monotonic';setup_budget_ms=120000;read_budget_ms=5000;
      setup_selection='main-dev-facts-120s';development_setup_opt_in=$true;entry_context_current=$true;root_identity_current=$true;
      closure_absent=$true;attempt_identity_current=$true;within_accepted_deadline=$true;atomic_snapshot=$false;native_authority=$false;render_authority=$false}
}
function New-Facts {
    [pscustomobject]@{kind='development-observed-facts';nonce=$nonce;attempt_pid='1234';attempt_start='5678';document_id=$expected.document;
      order=$order;current_index=0;current_page_id=$order[0];instance='1';begin_epoch='0';end_epoch='0';required_metadata_validated=$true;
      required_connections_installed=$true;begin_ms=0;end_ms=1;request_accepted_ms=119999;delivered_ms=120010;atomic_snapshot=$false;
      native_authority=$false;render_authority=$false;development_setup_opt_in=$true;setup_budget_ms=120000;setup_selection='main-dev-facts-120s'}
}
function Expand([string]$text){$text}
function Require($r){if($r.timeout -or $r.exit -ne 0){throw 'mock transport refused'}}
function Hash([string]$path){(Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant()}
function ObservationSSH([string]$command){
    $script:calls++
    if($script:calls -eq 1){return @{timeout=$false;exit=0;stdout="1234 5678 19 20`n"}}
    if($script:calls -eq 2){
        if($script:mode -ceq 'transport'){return @{timeout=$true;exit=0;stdout=''}}
        if($script:mode -ceq 'success'){return @{timeout=$false;exit=0;stdout=((New-Facts)|ConvertTo-Json -Compress)}}
        if($script:mode -ceq 'missing'){return @{timeout=$false;exit=0;stdout="absent`n"}}
        if($script:mode -ceq 'malformed'){return @{timeout=$false;exit=0;stdout="present`n"+'{'}}
        return @{timeout=$false;exit=0;stdout="present`n"+($script:refusal|ConvertTo-Json -Compress)}
    }
    return @{timeout=$false;exit=$(if($script:mode -ceq 'final-generation'){90}else{0});stdout=''}
}
try{
    foreach($mode in @('success','refusal','context','missing','malformed','wrong-tuple','transport','final-generation','deadline','callback-mismatch')){
        $script:mode=$mode;$script:calls=0;$script:refusal=New-Refusal
        $record=@{facts_verified=$false;candidate_generation_verified=$false;live_refusal=$null;live_refusal_callback_match=$false}
        $observationClock=[pscustomobject]@{ElapsedMilliseconds=$(if($mode -ceq 'deadline'){150000}else{500})}
        $cb=[pscustomobject]@{nonce=$nonce;stage='facts-entry-read-refused';application_thread=$true;engine_thread=$true}
        if($mode -ceq 'success'){$cb.stage='facts-observed-no-change-during-read'}
        if($mode -cin @('context','callback-mismatch')){
            $script:refusal.entry_stage='facts-entry-delivery-refused';$script:refusal.reader_result_had_facts=$true;$script:refusal.refusal_path='entry-completion-boundary';$script:refusal.entry_context_current=$false
            if($mode -ceq 'context'){$cb.stage='facts-entry-delivery-refused';$cb.engine_thread=$false}
        }
        if($mode -ceq 'wrong-tuple'){$script:refusal.attempt_start='5679'}
        $observed=@{stdout=($cb|ConvertTo-Json -Compress)}
        $threw=$false;try{. $branch}catch{$threw=$true}
        Check ($threw -eq ($mode -cne 'success')) "live branch $mode"
        Check ($record.facts_verified -eq ($mode -ceq 'success') -and $record.candidate_generation_verified -eq ($mode -ceq 'success')) "no refusal promotion $mode"
        Check ($record.live_refusal_callback_match -eq ($mode -cin @('refusal','context'))) "distinct diagnostic match $mode"
    }
    # Exact recovery body with transport mocks; historical diagnostic cannot alter live flags.
    $start=$source.IndexOf('}finally{')+10
    $recovery=[scriptblock]::Create($source.Substring($start,$source.TrimEnd().Length-$start-1))
    function Start-Sleep {param($Seconds,$Milliseconds)}
    function Native {param($program,$arguments)
        $destination=$arguments[-1]
        if($destination.EndsWith('attempt.identity')){[IO.File]::WriteAllText($destination,"1234 5678`n")}
        if($destination.EndsWith('callback-final.json')){[IO.File]::WriteAllText($destination,([pscustomobject]@{nonce=$nonce;stage='facts-entry-read-refused';application_thread=$true;engine_thread=$true}|ConvertTo-Json -Compress))}
        return @{timeout=$false;exit=0;stdout=''}
    }
    function SSH([string]$command){
        if($command.Contains('stock-restored-and-exact-stage-removed')){
            Check ($command.Contains('diagnostics.json refusal.json facts-publisher')) 'exact cleanup includes refusal only'
            $script:cleanup=$true;return @{timeout=$false;exit=0;stdout=''}
        }
        if($command.Contains('stock-verified')){return @{timeout=$false;exit=0;stdout=''}}
        if($command.Contains("@ROOT@/refusal.json")){
            if($script:mode -ceq 'transport'){return @{timeout=$true;exit=0;stdout=''}}
            if($script:mode -ceq 'missing'){return @{timeout=$false;exit=0;stdout="absent`n"}}
            if($script:mode -ceq 'malformed'){return @{timeout=$false;exit=0;stdout="present`n"+'{'}}
            return @{timeout=$false;exit=0;stdout="present`n"+((New-Refusal)|ConvertTo-Json -Compress)}
        }
        if($command.Contains("printf 'present")){
            return @{timeout=$false;exit=0;stdout=$(if($command.Contains('@ROOT@/callback.json')){"present`n"}else{"absent`n"})}
        }
        return @{timeout=$false;exit=0;stdout=''}
    }
    foreach($mode in @('historical-only','missing','malformed','transport')){
        $script:mode=$mode;$script:cleanup=$false
        $record=@{arm_intent=$true;restored=$false;cleanup_verified=$false;callback_verified=$false;facts_verified=$false;
            candidate_generation_verified=$false;gate_evidence=@{};final_callback_collected=$false;final_refusal=$null;final_refusal_callback_match=$false}
        $threw=$false;try{. $recovery|Out-Null}catch{$threw=$true}
        Check $threw "refused trial remains refused $mode"
        Check ($script:cleanup -eq ($mode -cne 'transport')) "recovery/collection ordering $mode"
        Check (-not $record.facts_verified -and -not $record.candidate_generation_verified -and -not $record.callback_verified) "no historical upgrade $mode"
        Check ($record.final_refusal_callback_match -eq ($mode -ceq 'historical-only')) "historical diagnostic match $mode"
    }
}finally{Remove-Item -LiteralPath $packet -Recurse -Force}
Write-Output "PASS $count future exact live/recovery assertions; mocked transport only"
