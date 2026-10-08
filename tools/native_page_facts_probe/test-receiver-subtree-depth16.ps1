$ErrorActionPreference='Stop'
. "$PSScriptRoot/test-receiver-subtree-cap2048.ps1"
$startDepth=$checks
function CompletionDepth16($v){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $true $false 2048 16}
function RefusalDepth16($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $true $false 2048 16}
$cDepth=$c2048|ConvertTo-Json|ConvertFrom-Json;$cDepth.discovery_scope='receiver-subtree-capture-unqualified-v5'
Check (CompletionDepth16 $cDepth) 'selected16 completion'
Check (-not(Completion2048 $cDepth)) 'v4 refuses v5'
Check (-not(CompletionDepth16 $c2048)) 'v5 refuses v4'
$depth16=$depth|ConvertTo-Json|ConvertFrom-Json;$depth16.discovery_scope='receiver-subtree-capture-unqualified-v5';$depth16.topology_depth=16;$depth16.topology_queue_size=20;$depth16.visited_items=20
Check (RefusalDepth16 $depth16) 'depth17 refusal at16'
$bad=$depth16|ConvertTo-Json|ConvertFrom-Json;$bad.topology_depth=17
Check (-not(RefusalDepth16 $bad)) 'depth16 limit strict'
$old8=$depth16|ConvertTo-Json|ConvertFrom-Json;$old8.discovery_scope='receiver-subtree-capture-unqualified-v4';$old8.topology_depth=8;$old8.visited_items=940;$old8.topology_queue_size=1162;$old8.scene_candidates=2
Check (Refusal2048 $old8) 'historical2048 depth8 preserved'
Check (-not(RefusalDepth16 $old8)) 'new selection refuses oldscope'
$bad=$old8|ConvertTo-Json|ConvertFrom-Json;$bad.discovery_scope='receiver-subtree-capture-unqualified-v5'
Check (-not(RefusalDepth16 $bad)) 'olddepth8 cannot claim depth16 overflow'
$bad=$depth16|ConvertTo-Json|ConvertFrom-Json;$bad.discovery_scope='receiver-subtree-capture-unqualified-v4'
Check (-not(Refusal2048 $bad)) 'oldscope cannot acquire depth16'
$item16=$over2048|ConvertTo-Json|ConvertFrom-Json;$item16.discovery_scope='receiver-subtree-capture-unqualified-v5';$item16.topology_depth=15
Check (RefusalDepth16 $item16) 'itembound may refuse beforedepth16'
$item16.topology_depth=16
Check (-not(RefusalDepth16 $item16)) 'depthbound takes precedence at16'
$configDepth=Get-CaptureObservationBuildConfig $expected $false $true $false 2048 16
Check ($configDepth.Contains('config.developmentReceiverSubtreeDepthCap=16;') -and $configDepth.Contains('config.developmentReceiverSubtreeItemCap=2048;')) 'explicitdepth config'
Check (-not $config2048.Contains('config.developmentReceiverSubtreeDepthCap=16;')) 'oldconfig depth8 unchanged'
foreach($profile in @(@($false,$false,2048,16),@($true,$true,2048,16),@($true,$false,1024,16),@($true,$false,0,16),@($true,$false,2048,8),@($true,$false,2048,17))){
 $threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $profile[0] $profile[1] $profile[2] $profile[3]|Out-Null}catch{$threw=$true};Check $threw 'invaliddepth config'
 Check (-not(Test-CaptureObservationCompletion $cDepth $nonce '1234' '5678' '11' '22' $document @($page) $profile[0] $profile[1] $profile[2] $profile[3])) 'invaliddepth completion'
 Check (-not(Test-CaptureOwnerRefusal $depth16 $nonce '1234' '5678' '11' '22' $false $profile[0] $profile[1] $profile[2] $profile[3])) 'invaliddepth refusal'
}
$ReceiverSubtreeCapture512=$false;$ReceiverSubtreeItemCap=2048;$ReceiverSubtreeDepthCap=16;$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap) if($args.Count -or $focus -or -not $subtree -or $profile512 -or $itemCap -ne 2048 -or $depthCap -ne 16){throw 'Wrongdepth16 route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actualdepth16 collector route'
Write-Output "PASS receiver subtree depth16 focused consumer $($checks-$startDepth) checks; historical depths preserved"