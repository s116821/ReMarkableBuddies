$ErrorActionPreference='Stop'
. "$PSScriptRoot/test-receiver-subtree-depth16.ps1"
$start4096=$checks
function Completion4096($v){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $true $false 4096 16}
function Refusal4096($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $true $false 4096 16}
$c4096=$cDepth|ConvertTo-Json|ConvertFrom-Json;$c4096.discovery_scope='receiver-subtree-capture-unqualified-v6'
Check (Completion4096 $c4096) 'selected4096 completion'
Check (-not(CompletionDepth16 $c4096)) 'v5 refuses v6'
Check (-not(Completion4096 $cDepth)) 'v6 refuses v5'
$over4096=$over2048|ConvertTo-Json|ConvertFrom-Json;$over4096.discovery_scope='receiver-subtree-capture-unqualified-v6';$over4096.topology_child_count=4096;$over4096.topology_depth=15
Check (Refusal4096 $over4096) '4097 append refusal'
$bad=$over4096|ConvertTo-Json|ConvertFrom-Json;$bad.topology_child_count=4095
Check (-not(Refusal4096 $bad)) '4096 append cannot claim overflow'
foreach($name in @('visited_items','topology_queue_size')){$bad=$over4096|ConvertTo-Json|ConvertFrom-Json;$bad.$name=4097;Check (-not(Refusal4096 $bad)) "4096 bound $name"}
$historic2048=$item16|ConvertTo-Json|ConvertFrom-Json;$historic2048.topology_depth=12;$historic2048.topology_queue_size=2048;$historic2048.topology_child_count=2;$historic2048.visited_items=1824;$historic2048.scene_candidates=2
Check (RefusalDepth16 $historic2048) 'historicalv5 item2048 bound retained'
Check (-not(Refusal4096 $historic2048)) 'v6 rejects historicalv5 scope'
$bad=$historic2048|ConvertTo-Json|ConvertFrom-Json;$bad.discovery_scope='receiver-subtree-capture-unqualified-v6'
Check (-not(Refusal4096 $bad)) '2050 frontier does not overflow4096'
$config4096=Get-CaptureObservationBuildConfig $expected $false $true $false 4096 16
Check ($config4096.Contains('config.developmentReceiverSubtreeItemCap=4096;') -and $config4096.Contains('config.developmentReceiverSubtreeDepthCap=16;')) 'explicit4096 config'
foreach($profile in @(@($true,$false,4096,0),@($true,$false,4096,8),@($true,$true,4096,16),@($false,$false,4096,16),@($true,$false,4095,16))){
 $threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $profile[0] $profile[1] $profile[2] $profile[3]|Out-Null}catch{$threw=$true};Check $threw 'invalid4096 config'
 Check (-not(Test-CaptureObservationCompletion $c4096 $nonce '1234' '5678' '11' '22' $document @($page) $profile[0] $profile[1] $profile[2] $profile[3])) 'invalid4096 completion'
 Check (-not(Test-CaptureOwnerRefusal $over4096 $nonce '1234' '5678' '11' '22' $false $profile[0] $profile[1] $profile[2] $profile[3])) 'invalid4096 refusal'
}
$ReceiverSubtreeItemCap=4096;$ReceiverSubtreeDepthCap=16;$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap) if($args.Count -or $focus -or -not $subtree -or $profile512 -or $itemCap -ne 4096 -or $depthCap -ne 16){throw 'Wrong4096 route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actual4096 collector route'
Write-Output "PASS receiver subtree4096 focused consumer $($checks-$start4096) checks; historical bounds preserved"