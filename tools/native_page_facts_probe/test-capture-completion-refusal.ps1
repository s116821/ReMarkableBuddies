$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-owner-refusal-proof.ps1"
. "$PSScriptRoot/capture-observation-build-config.ps1"
$count=0
function Check($value,$label){if(-not $value){throw $label};$script:count++}
$nonce='0123456789abcdef0123456789abcdef'
$d=[ordered]@{kind='development-capture-completion-refusal';version=1;reason='allowed-refused';capture_accepted_ms=0;baseline_ms=1;post_read_ms=2;failure_ms=3;effective_deadline_ms=5000}
$c=[ordered]@{nonce=$nonce;stage='capture-observation-completion-refused';application_thread=$true;engine_thread=$true;completion_refusal=$d}
function Raw {ConvertTo-Json -InputObject $c -Depth 5 -Compress}
foreach($reason in @('allowed-deadline','allowed-refused','identity-refused','epoch-changed','token-refused','facts-request-present','facts-request-tmp-present')){
 $d.reason=$reason;Check ($null -ne (Get-CaptureCompletionRefusal (Raw) $nonce)) 'fixed reason accepted'
}
$d.reason='inferred-deadline';Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'unknown reason refused';$d.reason='allowed-refused'
$raw=Raw
Check ($null -eq (Get-CaptureCompletionRefusal ($raw.Replace('"version":1','"version":1,"version":1')) $nonce)) 'nested duplicate refused'
Check ($null -eq (Get-CaptureCompletionRefusal ($raw.Replace('"nonce":','"nonce":"duplicate","nonce":')) $nonce)) 'outer duplicate refused'
$d.failure_ms='3';Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'string clock refused';$d.failure_ms=3
$c.stage='facts-entry-observed';Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'success stage refused';$c.stage='capture-observation-completion-refused'
$c.stage=@('capture-observation-completion-refused');Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'array stage refused';$c.stage='capture-observation-completion-refused'
$d.kind=@('development-capture-completion-refusal');Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'array kind refused';$d.kind='development-capture-completion-refusal'
$d.extra=$false;Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'unknown field refused';$d.Remove('extra')
Check ($null -eq (Get-CaptureCompletionRefusal ($raw+' '*1024) $nonce)) 'oversize refused'
# Historical v1 remains exact8; selected v2 is exact10 with fixed nullable labels.
$d.version=2;$d.allowed_predicate='invalidated-before';$d.allowed_active_reason=$null
Check ($null -ne (Get-CaptureCompletionRefusal (Raw) $nonce)) 'v2 cached invalidation/null accepted'
$d.allowed_predicate='active-owner';$d.allowed_active_reason='capture-context'
Check ($null -ne (Get-CaptureCompletionRefusal (Raw) $nonce)) 'v2 fixed cached labels accepted'
$d.allowed_predicate='unknown';Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'v2 unknown predicate refused';$d.allowed_predicate='active-owner'
$d.allowed_active_reason=@('capture-context');Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'v2 array active reason refused'
$d.allowed_active_reason='unknown';Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'v2 unknown active reason refused';$d.allowed_active_reason=$null
$d.Remove('allowed_predicate');Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'v2 missing nullable field refused';$d.allowed_predicate=$null
$d.version=1;Check ($null -eq (Get-CaptureCompletionRefusal (Raw) $nonce)) 'v1 rejects extra v2 labels'
$d.Remove('allowed_predicate');$d.Remove('allowed_active_reason');Check ($null -ne (Get-CaptureCompletionRefusal (Raw) $nonce)) 'historical exact8 v1 still accepted'
$expected=@{nonce=$nonce;document='00000000-0000-4000-8000-000000000001';order=@(2..7|ForEach-Object {'00000000-0000-4000-8000-'+$_.ToString('000000000000')})}
$old=Get-CaptureObservationBuildConfig $expected $false $true $false 4096 32 $true $true $true $true
$new=Get-CaptureObservationBuildConfig $expected $false $true $false 4096 32 $true $true $true $true $true
Check (-not $old.Contains('developmentCaptureCompletionRefusalDiagnostics')) 'default off config unchanged'
Check ($new.Replace("    config.developmentCaptureCompletionRefusalDiagnostics=true;`n",'') -ceq $old) 'only selected config flag added'
$threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $false $false 0 0 $false $false $false $false $true|Out-Null}catch{$threw=$true};Check $threw 'requires selected v11'
$operator=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-capture-observation-source.ps1'))
$line=($operator -split "`n"|Where-Object {$_ -like 'function Expand*'})
. ([scriptblock]::Create($line))
$remote='/run/owned-fixture';$rollback='owned';$stock=@{stock_pid=1;stock_start='2'};$fixtureCheck='true'
$CaptureCompletionRefusalDiagnostics=$false;Check ((Expand '-le @CALLBACKCAP@') -ceq '-le 256') 'actual default callback cap'
$CaptureCompletionRefusalDiagnostics=$true;Check ((Expand '-le @CALLBACKCAP@') -ceq '-le 1024') 'actual selected callback cap'
Check (($operator -split '@CALLBACKCAP@').Count -eq 4) 'both callback reads bounded'
Write-Output "PASS completion refusal $count focused consumer checks; diagnostics confer no authority"
