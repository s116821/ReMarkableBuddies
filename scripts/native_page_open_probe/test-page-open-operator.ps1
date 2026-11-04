$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'page-open-diagnostics-proof.ps1')
. (Join-Path $PSScriptRoot '../native_entry_probe/page-open-proof.ps1')
$script:cases=0
function New-Diagnostics {
    [pscustomobject]@{terminal_stage='open-observed';access_anchor='setup-arm-observation';elapsed_ms=1500;page_open_trial=[pscustomobject]@{enabled=$true;setup_gate_enabled=$true;target_observed=$true;render_authority=$false;native_api_qualified=$false;native_call_attempted=$true;native_call_returned=$true;arm_accepted_at_ms=1000}}
}
function Check($value,[bool]$expected,[string]$name){
    $script:cases++
    if((Test-PageOpenDiagnostics $value) -ne $expected){throw "Diagnostics case: $name"}
}
Check (New-Diagnostics) $true 'native call'
$d=New-Diagnostics;$d.page_open_trial.native_call_attempted=$false;$d.page_open_trial.native_call_returned=$false;Check $d $true 'verified noop'
Check $null $false 'absent'
foreach($field in @('enabled','setup_gate_enabled','target_observed','render_authority','native_api_qualified','native_call_attempted','native_call_returned')){
    foreach($bad in @('true',1,$null)){$d=New-Diagnostics;$d.page_open_trial.$field=$bad;Check $d $false "typed $field"}
    $d=New-Diagnostics;$d.page_open_trial.PSObject.Properties.Remove($field);Check $d $false "missing $field"
    $d=New-Diagnostics;$d.page_open_trial.$field=-not $d.page_open_trial.$field;Check $d $false "wrong $field"
}
foreach($field in @('terminal_stage','access_anchor')){
    foreach($bad in @('resolved','open-observed ','library-ready-observation',1,$null)){$d=New-Diagnostics;$d.$field=$bad;Check $d $false "wrong $field"}
}
foreach($bad in @(-1,20000,20001,'1000',1000.0,$null)){$d=New-Diagnostics;$d.page_open_trial.arm_accepted_at_ms=$bad;Check $d $false 'arm deadline/type'}
foreach($bad in @(999,6000,6001,'1500',1500.0,$null)){$d=New-Diagnostics;$d.elapsed_ms=$bad;Check $d $false 'access deadline/type'}
foreach($good in @(1000,5999)){$d=New-Diagnostics;$d.elapsed_ms=$good;Check $d $true 'access boundary'}
$d=New-Diagnostics;$d.page_open_trial.arm_accepted_at_ms=19999;$d.elapsed_ms=24998;Check $d $true 'latest valid gate/access'
foreach($bad in @($null,[pscustomobject]@{})){$d=New-Diagnostics;$d|Add-Member creation_trial $bad;Check $d $false 'creation forbidden even null'}
$operator=Join-Path $PSScriptRoot 'run-qt-page-open-one-shot.ps1'
$tokens=$null;$errors=$null;[void][Management.Automation.Language.Parser]::ParseFile($operator,[ref]$tokens,[ref]$errors)
if($errors.Count){throw ($errors|Out-String)}
# Execute the exact live callback branch with transport mocks. No complete operator execution.
$source=[IO.File]::ReadAllText($operator)
$begin=$source.IndexOf('            [IO.File]::WriteAllText((Join-Path $packet ''callback.json'')')
$end=$source.IndexOf('            break',$begin)
if($begin -lt 0 -or $end -lt 0){throw 'Live branch extraction failed'}
$branch=[scriptblock]::Create($source.Substring($begin,$end-$begin))
$nonce='604d9d8e17c046fe84fea4e44e608165'
$packet=Join-Path ([IO.Path]::GetTempPath()) ('open-proof-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory $packet)
function Expand([string]$text){return $text}
function Require($result){if($result.timeout -or $result.exit -ne 0){throw 'mock refusal'}}
function ObservationSSH([string]$command){
    $script:transportCalls++
    if($script:failure -ceq 'budget'){throw 'budget exhausted'}
    if($script:failure -ceq 'generation' -and $script:transportCalls -eq 1){return @{timeout=$false;exit=90;stdout=''}}
    if($script:failure -ceq 'transport'){return @{timeout=$true;exit=0;stdout=''}}
    return @{timeout=$false;exit=0;stdout=($script:diag|ConvertTo-Json -Depth 5 -Compress)}
}
try{
    foreach($mode in @('success','noop','legacy','diagnostics','generation','budget','transport')){
        $record=@{page_open_verified=$false;candidate_generation_verified=$false};$script:transportCalls=0;$script:failure=$mode
        $script:diag=New-Diagnostics
        $callback=[pscustomobject]@{nonce=$nonce;stage='open-observed';application_thread=$true;engine_thread=$true;helper_available=$true;controller_available=$true}
        if($mode -ceq 'legacy'){$callback.stage='resolved'}
        if($mode -ceq 'diagnostics'){$script:diag.terminal_stage='open-refused'}
        if($mode -ceq 'noop'){$script:diag.page_open_trial.native_call_attempted=$false;$script:diag.page_open_trial.native_call_returned=$false}
        $observed=@{stdout=($callback|ConvertTo-Json -Compress)}
        $threw=$false;try{. $branch}catch{$threw=$true}
        $expected=$mode -cin @('success','noop')
        if($record.page_open_verified -ne $expected){throw "Live branch $mode"}
        if(($mode -cin @('generation','budget','transport')) -ne $threw){throw "Live transport refusal $mode"}
        $script:cases++
    }
    # Execute the exact mandatory recovery block with transport/file mocks.
    $recoveryStart=$source.IndexOf('}finally{')+10
    $recovery=[scriptblock]::Create($source.Substring($recoveryStart,$source.Length-$recoveryStart-2))
    function Start-Sleep {param($Seconds,$Milliseconds)}
    function Native {param($program,$arguments) return @{exit=0;timeout=$false;stdout=''}}
    function SSH([string]$command){
        $script:recoveryCommands+=$command
        if($command.Contains('stock-restored-and-exact-stage-removed')){
            $script:cleanupCalled=$true
            return @{exit=$(if($script:recoveryMode -ceq 'cleanup-failure'){90}else{0});timeout=$false;stdout=''}
        }
        if($command.Contains('stock-verified')){return @{exit=$(if($script:recoveryMode -ceq 'restore-failure'){90}else{0});timeout=$false;stdout=''}}
        if($command.Contains("printf 'present")){
            if($script:recoveryMode -ceq 'gate-failure' -and $command.Contains('@NAME@')){throw 'unused placeholder'}
            if($script:recoveryMode -ceq 'gate-failure' -and $command.Contains('open-waiting')){return @{exit=90;timeout=$false;stdout=''}}
            return @{exit=0;timeout=$false;stdout="absent`n"}
        }
        return @{exit=0;timeout=$false;stdout=''}
    }
    foreach($mode in @('success','proof-refusal','restore-failure','gate-failure','cleanup-failure')){
        $script:recoveryMode=$mode;$script:cleanupCalled=$false;$script:recoveryCommands=@()
        $record=@{arm_intent=$true;restored=$false;cleanup_verified=$false;callback_verified=$true;page_open_verified=($mode -cne 'proof-refusal');gate_evidence=@{}}
        $threw=$false;try{. $recovery|Out-Null}catch{$threw=$true}
        if($threw -ne ($mode -cne 'success')){throw "Recovery result $mode"}
        if($script:cleanupCalled -ne ($mode -cin @('success','proof-refusal','cleanup-failure'))){throw "Recovery cleanup ordering $mode"}
        if(-not(Test-Path (Join-Path $packet 'operator-receipt.json'))){throw "Recovery receipt $mode"}
        if(-not($script:recoveryCommands[0].Contains('start --no-block'))){throw "Singleton restore trigger $mode"}
        $script:cases++
    }
}finally{Remove-Item -LiteralPath $packet -Recurse -Force}
Write-Output "PASS $script:cases diagnostics/live-branch cases; parser clean; no transport/device"
