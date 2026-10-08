$ErrorActionPreference='Stop'
. "$PSScriptRoot/test-receiver-subtree-cap512.ps1"
$start1024=$checks
function Completion1024($v){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $true $false 1024}
function Refusal1024($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $true $false 1024}
$c1024=$c|ConvertTo-Json|ConvertFrom-Json;$c1024.discovery_scope='receiver-subtree-capture-unqualified-v3'
Check (Completion1024 $c1024) 'explicit1024 completion'
Check (-not(Completion $c1024)) 'legacy256 refuses v3'
Check (-not(Completion512 $c1024)) 'legacy512 refuses v3'
Check (-not(Completion1024 $c)) '1024 refuses v1'
Check (-not(Completion1024 $c512)) '1024 refuses v2'
$complete1024=$complete512|ConvertTo-Json|ConvertFrom-Json;$complete1024.discovery_scope='receiver-subtree-capture-unqualified-v3';$complete1024.visited_items=1024
Check (Refusal1024 $complete1024) 'complete1024 no match'
Check (-not(Refusal512 $complete1024)) '512 scope cannot adopt1024'
$over1024=$over|ConvertTo-Json|ConvertFrom-Json;$over1024.discovery_scope='receiver-subtree-capture-unqualified-v3';$over1024.topology_child_count=1024
Check (Refusal1024 $over1024) '1025 append refusal'
$fits1024=$over1024|ConvertTo-Json|ConvertFrom-Json;$fits1024.topology_child_count=1023
Check (-not(Refusal1024 $fits1024)) 'exact1024 enqueue cannot claim overflow'
foreach($field in @('visited_items','topology_queue_size')){$bad=$over1024|ConvertTo-Json|ConvertFrom-Json;$bad.$field=1025;Check (-not(Refusal1024 $bad)) "1024 bounds $field"}
$historical512=$over|ConvertTo-Json|ConvertFrom-Json;$historical512.topology_queue_size=511;$historical512.topology_child_count=6;$historical512.topology_depth=4;$historical512.visited_items=288;$historical512.scene_candidates=2
Check (Refusal512 $historical512) 'historical512 frontier retained'
Check (-not(Refusal1024 $historical512)) '1024 refuses historical scope'
$fits1024=$historical512|ConvertTo-Json|ConvertFrom-Json;$fits1024.discovery_scope='receiver-subtree-capture-unqualified-v3'
Check (-not(Refusal1024 $fits1024)) 'historical517 frontier does not overflow1024'
$config1024=Get-CaptureObservationBuildConfig $expected $false $true $false 1024
Check ($config1024.Contains('config.developmentReceiverSubtreeItemCap=1024;') -and -not $config1024.Contains('config.developmentReceiverSubtreeCapture512=true;')) 'explicit1024 config'
foreach($profile in @(@($false,$false,1024),@($true,$true,1024),@($true,$false,768),@($true,$false,-1))){
 $threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $profile[0] $profile[1] $profile[2]|Out-Null}catch{$threw=$true};Check $threw 'invalid profile config'
 Check (-not(Test-CaptureObservationCompletion $c1024 $nonce '1234' '5678' '11' '22' $document @($page) $profile[0] $profile[1] $profile[2])) 'invalid completion profile'
 Check (-not(Test-CaptureOwnerRefusal $over1024 $nonce '1234' '5678' '11' '22' $false $profile[0] $profile[1] $profile[2])) 'invalid refusal profile'
}
$ReceiverSubtreeCapture512=$false;$ReceiverSubtreeItemCap=1024;$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap=0) if($depthCap -or $args.Count -or $focus -or -not $subtree -or $profile512 -or $itemCap -ne 1024){throw 'Wrong1024 profile route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actual1024 owner collector route'
Write-Output "PASS receiver subtree1024 focused consumer $($checks-$start1024) checks; historical scope bounds preserved"