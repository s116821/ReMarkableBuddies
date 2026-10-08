$ErrorActionPreference='Stop'
. "$PSScriptRoot/test-receiver-subtree-capture.ps1"
$startChecks=$checks
function Completion512($v){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $true $true}
function Refusal512($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $true $true}
$c512=$c|ConvertTo-Json|ConvertFrom-Json;$c512.discovery_scope='receiver-subtree-capture-unqualified-v2'
Check (Completion512 $c512) 'selected512 completion'
Check (-not(Completion $c512)) 'old bool scope refuses512 completion'
Check (-not(Completion512 $c)) '512 selection refuses old completion'
Check (-not(Test-CaptureObservationCompletion $c512 $nonce '1234' '5678' '11' '22' $document @($page) $false $true)) '512 cannot bypass receiver selection'
$complete512=$v|ConvertTo-Json|ConvertFrom-Json;$complete512.discovery_scope='receiver-subtree-capture-unqualified-v2';$complete512.visited_items=512
Check (Refusal512 $complete512) 'complete512 no matched scene'
Check (-not(Refusal $complete512)) 'legacy selection rejects512 scope'
$over=$bound|ConvertTo-Json|ConvertFrom-Json;$over.discovery_scope='receiver-subtree-capture-unqualified-v2';$over.topology_child_count=512
Check (Refusal512 $over) '513 enqueue refuses before values'
$fits=$over|ConvertTo-Json|ConvertFrom-Json;$fits.topology_child_count=511
Check (-not(Refusal512 $fits)) '512 enqueue cannot claim overflow'
$tooMany=$over|ConvertTo-Json|ConvertFrom-Json;$tooMany.visited_items=513
Check (-not(Refusal512 $tooMany)) '512 visit bound strict'
$tooMany=$over|ConvertTo-Json|ConvertFrom-Json;$tooMany.topology_queue_size=513
Check (-not(Refusal512 $tooMany)) '512 queue bound strict'
Check (Refusal $bound) 'legacy256 overflow preserved'
Check (-not(Refusal512 $bound)) '512 does not reinterpret old256 overflow'
$config512=Get-CaptureObservationBuildConfig $expected $false $true $true
Check ($config512.Contains('config.developmentReceiverSubtreeCapture512=true;') -and $config512.Contains('config.developmentReceiverSubtreeCapture=true;')) 'explicit512 config'
Check (-not $config.Contains('config.developmentReceiverSubtreeCapture512=true;')) 'legacy bool256 config preserved'
$threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $false $true|Out-Null}catch{$threw=$true};Check $threw 'unselected512 config refused'
$ReceiverSubtreeCapture512=$true;$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap=0,[int]$depthCap=0,[bool]$allowUnfocused=$false) if($allowUnfocused -or $depthCap -or $args.Count -or $focus -or -not $subtree -or -not $profile512 -or $itemCap){throw 'Wrong512 scope route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actual512 owner collector route'
Write-Output "PASS receiver subtree512 focused consumer $($checks-$startChecks) checks; legacy scoped checks preserved"