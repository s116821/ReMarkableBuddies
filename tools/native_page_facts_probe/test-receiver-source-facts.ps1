$ErrorActionPreference='Stop'
. "$PSScriptRoot/receiver-source-facts-proof.ps1"
. "$PSScriptRoot/receiver-source-facts-build-config.ps1"
$count=0
function Check($value,$message){if(-not $value){throw $message};$script:count++}
$n='0123456789abcdef0123456789abcdef';$i=@{attempt_pid='42';attempt_start='100';root_device='19';root_inode='200'}
$e=@{nonce=$n;document='00000000-0000-4000-8000-000000000001';order=@(2..7|ForEach-Object {'00000000-0000-4000-8000-'+$_.ToString('000000000000')})}
$v=[ordered]@{kind='development-receiver-source-facts';version=1;scope='receiver-source-facts-unqualified-v1';nonce=$n;attempt_pid='42';attempt_start='100';root_device='19';root_inode='200';document_id=$e.document;page_id=$e.order[0];page_index=0;page_count=6;order=$e.order;alias_matches=$true;forward_reverse_mapping_matches=$true;observed_order=$true;accepted_ms=100;read_begin_ms=101;read_end_ms=102;effective_deadline_ms=5100;begin_epoch='1';end_epoch='1';atomic_snapshot=$false;native_authority=$false;render_authority=$false;ui_acknowledged=$false;delivered_ms=103}
function Valid {Test-ReceiverSourceFacts (ConvertFrom-ReceiverSourceFactsRaw ($v|ConvertTo-Json -Depth 4 -Compress)) $n $i $e}
Check (Valid) 'exact27 positive'
foreach($name in @('kind','version','page_count','observed_order','native_authority','page_index','accepted_ms','root_inode','order')){
 $prior=$v[$name];$v[$name]=@('bad');Check (-not(Valid)) "typed $name refused";$v[$name]=$prior
}
$v.delivered_ms=5100;Check (-not(Valid)) 'deadline boundary refused';$v.delivered_ms=103
$v.read_end_ms=99;Check (-not(Valid)) 'time inversion refused';$v.read_end_ms=102
$v.forward_reverse_mapping_matches=$false;Check (-not(Valid)) 'reverse mismatch refused';$v.forward_reverse_mapping_matches=$true
$v.end_epoch='2';Check (-not(Valid)) 'epoch changed refused';$v.end_epoch='1'
$v.extra=$false;Check (-not(Valid)) 'unknown field refused';$v.Remove('extra')
$raw=$v|ConvertTo-Json -Depth 4 -Compress
Check ($null -eq (ConvertFrom-ReceiverSourceFactsRaw ($raw.Replace('"version":1','"version":1,"version":1')))) 'duplicate refused'
$c=[pscustomobject]@{nonce=$n;stage='receiver-source-facts-observed-unqualified';application_thread=$true;engine_thread=$true;source_facts_reader_stage='facts-observed-no-change-during-read'}
Check (Test-ReceiverSourceFactsCallback $c $n) 'separate callback exact5'
$c.source_facts_reader_stage='';Check (-not(Test-ReceiverSourceFactsCallback $c $n)) 'empty reader stage cannot claim observed';$c.source_facts_reader_stage='facts-observed-no-change-during-read'
$c.stage=@('receiver-source-facts-observed-unqualified');Check (-not(Test-ReceiverSourceFactsCallback $c $n)) 'array callback stage refused'
$config=Get-ReceiverSourceFactsBuildConfig $e;$old=Get-CaptureObservationBuildConfig $e $false $true $false 4096 32 $true $true $true $true
Check ($config.Replace("    config.developmentReceiverSourceFacts=true;`n",'') -ceq $old) 'only sourcefacts flag config difference'
$operator=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-page-facts-diagnostic-source.ps1'));$line=($operator -split "`n"|Where-Object {$_ -like 'function Expand*'});. ([scriptblock]::Create($line))
$remote='/run/owned';$nonce=$n;$rollback='owned';$stock=@{stock_pid=42;stock_start='100'};$fixtureCheck='true';$ReceiverSourceFacts=$false
$payloadHash='0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
$payloadCheck=@($operator -split "`n"|Where-Object {$_.Contains("sha256sum '@ROOT@/payload.so'")})
Check ($payloadCheck.Count -eq 1 -and (Expand $payloadCheck[0]).TrimEnd().EndsWith((' = '+$payloadHash),[StringComparison]::Ordinal)) 'actual prearm payload checksum binds selected hash'
Check ((Expand '@REQUEST@ @PURPOSE@ @REQUESTCAP@ @CALLBACKCAP@ @PUBLISHER@') -ceq 'facts-request read-facts 128 256 facts-publisher') 'actual default protocol unchanged'
$ReceiverSourceFacts=$true;Check ((Expand '@REQUEST@ @PURPOSE@ @REQUESTCAP@ @CALLBACKCAP@ @PUBLISHER@') -ceq 'receiver-source-facts-request receiver-source-facts 256 512 receiver-source-facts-publisher') 'actual sourcefacts protocol'
Check ($operator.Contains('& $ManualActions $remote $nonce $publish')) 'Main manual action delegate before collection'
Check ($operator.Contains('$record.callback_verified=if($ReceiverSourceFacts){$false}')) 'old callback success remains false'
Check ($operator.Contains('final preservation unknown; retain exact root')) 'unknown final evidence holds cleanup'
$stageStart=$operator.IndexOf('$stageFiles=@');$stageEnd=$operator.IndexOf('foreach($name in $stageFiles)',$stageStart)
$ReceiverSourceFacts=$false;. ([scriptblock]::Create($operator.Substring($stageStart,$stageEnd-$stageStart)));Check (($stageFiles -join ',') -ceq 'launch.sh,restore.sh,native-probe.conf,check-waiting.sh,publish-facts-once.sh') 'default staging preserved'
$ReceiverSourceFacts=$true;. ([scriptblock]::Create($operator.Substring($stageStart,$stageEnd-$stageStart)));Check (-not($stageFiles -contains 'publish-facts-once.sh')) 'sourcefacts stages no facts publication script'
$manualStart=$operator.IndexOf('    if($ReceiverSourceFacts){', $operator.IndexOf('# Main performs positive waiting proof'))
$manualEnd=$operator.IndexOf('    $statusTimeoutUsed=', $manualStart)
$receiverBlock=$operator.Substring($manualStart,$manualEnd-$manualStart)
$payloadHash='hash';$script:pubCalls=0
function ObservationSSH($command){Check ($command.Contains('/receiver-source-facts-publisher')) 'actual delegate chooses sourcefacts publisher';$script:pubCalls++;@{exit=0;timeout=$false}}
function Require($value){if($value.exit -ne 0 -or $value.timeout){throw 'transport failed'}}
$ManualActions={param($root,$nonceText,$publish) Check ($root -ceq '/run/owned' -and $nonceText -ceq $n) 'actual hook positional arguments'; & $publish; $refused=$false;try{& $publish}catch{$refused=$true};Check $refused 'duplicate delegate invocation refused'}
. ([scriptblock]::Create($receiverBlock))
Check ($script:pubCalls -eq 1) 'one actual delegate publication only'
Write-Output "PASS receiver source facts $count focused checks"
