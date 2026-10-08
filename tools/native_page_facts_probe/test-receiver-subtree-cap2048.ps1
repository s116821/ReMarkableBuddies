$ErrorActionPreference='Stop'
. "$PSScriptRoot/test-receiver-subtree-cap1024.ps1"
$start2048=$checks
function Completion2048($v){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $true $false 2048}
function Refusal2048($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $true $false 2048}
$c2048=$c|ConvertTo-Json|ConvertFrom-Json;$c2048.discovery_scope='receiver-subtree-capture-unqualified-v4'
Check (Completion2048 $c2048) 'explicit2048 completion'
Check (-not(Completion $c2048)) 'legacy256 refuses v3'
Check (-not(Completion512 $c2048)) 'legacy512 refuses v3'
Check (-not(Completion2048 $c)) '2048 refuses v1'
Check (-not(Completion2048 $c512)) '2048 refuses v2'
$complete2048=$complete512|ConvertTo-Json|ConvertFrom-Json;$complete2048.discovery_scope='receiver-subtree-capture-unqualified-v4';$complete2048.visited_items=2048
Check (Refusal2048 $complete2048) 'complete2048 no match'
Check (-not(Refusal512 $complete2048)) '512 scope cannot adopt2048'
$over2048=$over|ConvertTo-Json|ConvertFrom-Json;$over2048.discovery_scope='receiver-subtree-capture-unqualified-v4';$over2048.topology_child_count=2048
Check (Refusal2048 $over2048) '2049 append refusal'
$fits2048=$over2048|ConvertTo-Json|ConvertFrom-Json;$fits2048.topology_child_count=2047
Check (-not(Refusal2048 $fits2048)) 'exact2048 enqueue cannot claim overflow'
foreach($field in @('visited_items','topology_queue_size')){$bad=$over2048|ConvertTo-Json|ConvertFrom-Json;$bad.$field=2049;Check (-not(Refusal2048 $bad)) "2048 bounds $field"}
$historical512=$over|ConvertTo-Json|ConvertFrom-Json;$historical512.topology_queue_size=511;$historical512.topology_child_count=6;$historical512.topology_depth=4;$historical512.visited_items=288;$historical512.scene_candidates=2
Check (Refusal512 $historical512) 'historical512 frontier retained'
Check (-not(Refusal2048 $historical512)) '2048 refuses historical scope'
$fits2048=$historical512|ConvertTo-Json|ConvertFrom-Json;$fits2048.discovery_scope='receiver-subtree-capture-unqualified-v4'
Check (-not(Refusal2048 $fits2048)) 'historical517 frontier does not overflow2048'
$config2048=Get-CaptureObservationBuildConfig $expected $false $true $false 2048
Check ($config2048.Contains('config.developmentReceiverSubtreeItemCap=2048;') -and -not $config2048.Contains('config.developmentReceiverSubtreeCapture512=true;')) 'explicit2048 config'
foreach($profile in @(@($false,$false,2048),@($true,$true,2048),@($true,$false,768),@($true,$false,-1))){
 $threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $profile[0] $profile[1] $profile[2]|Out-Null}catch{$threw=$true};Check $threw 'invalid profile config'
 Check (-not(Test-CaptureObservationCompletion $c2048 $nonce '1234' '5678' '11' '22' $document @($page) $profile[0] $profile[1] $profile[2])) 'invalid completion profile'
 Check (-not(Test-CaptureOwnerRefusal $over2048 $nonce '1234' '5678' '11' '22' $false $profile[0] $profile[1] $profile[2])) 'invalid refusal profile'
}
$historic1024=$over1024|ConvertTo-Json|ConvertFrom-Json;$historic1024.topology_queue_size=1024;$historic1024.topology_child_count=1;$historic1024.topology_depth=7;$historic1024.visited_items=817;$historic1024.scene_candidates=2
Check (Refusal1024 $historic1024) 'historical1024 frontier retained'
Check (-not(Refusal2048 $historic1024)) '2048 refuses historicalv3 scope'
$fits2048=$historic1024|ConvertTo-Json|ConvertFrom-Json;$fits2048.discovery_scope='receiver-subtree-capture-unqualified-v4'
Check (-not(Refusal2048 $fits2048)) '1025 frontier cannot claim2048 overflow'
Check (-not(Completion2048 $c1024)) '2048 refuses v3 completion'
$ReceiverSubtreeCapture512=$false;$ReceiverSubtreeItemCap=2048;$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap=0,[bool]$allowUnfocused=$false,[bool]$detailedIdentity=$false,[bool]$documentType=$false) if($documentType -or $detailedIdentity -or $allowUnfocused -or $depthCap -or $args.Count -or $focus -or -not $subtree -or $profile512 -or $itemCap -ne 2048){throw 'Wrong2048 profile route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actual2048 owner collector route'
Write-Output "PASS receiver subtree2048 focused consumer $($checks-$start2048) checks; historical scope bounds preserved"