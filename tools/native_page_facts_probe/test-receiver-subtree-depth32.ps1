$ErrorActionPreference='Stop'
. "$PSScriptRoot/test-receiver-subtree-cap4096.ps1"
$start32=$checks
function Completion32($v){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $true $false 4096 32}
function Refusal32($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $true $false 4096 32}
$c32=$c4096|ConvertTo-Json|ConvertFrom-Json;$c32.discovery_scope='receiver-subtree-capture-unqualified-v7'
Check (Completion32 $c32) 'v7 completion'
Check (-not(Completion4096 $c32)) 'v6 refuses v7'
Check (-not(Completion32 $c4096)) 'v7 refuses v6'
$d32=$depth16|ConvertTo-Json|ConvertFrom-Json;$d32.discovery_scope='receiver-subtree-capture-unqualified-v7';$d32.topology_depth=32;$d32.visited_items=34;$d32.topology_queue_size=34
Check (Refusal32 $d32) 'child33 depth32 refusal'
$bad=$d32|ConvertTo-Json|ConvertFrom-Json;$bad.topology_depth=33
Check (-not(Refusal32 $bad)) 'depth32 exact boundary'
$d6=$depth16|ConvertTo-Json|ConvertFrom-Json;$d6.discovery_scope='receiver-subtree-capture-unqualified-v6'
Check (Refusal4096 $d6) 'historicalv6 depth16 refusal'
Check (-not(Refusal32 $d6)) 'v7 refuses v6 depth16 record'
$bad=$d6|ConvertTo-Json|ConvertFrom-Json;$bad.discovery_scope='receiver-subtree-capture-unqualified-v7'
Check (-not(Refusal32 $bad)) 'depth16 cannot claim depth32 overflow'
$config32=Get-CaptureObservationBuildConfig $expected $false $true $false 4096 32
Check ($config32.Contains('config.developmentReceiverSubtreeDepthCap=32;') -and $config32.Contains('config.developmentReceiverSubtreeItemCap=4096;')) 'explicit32 config'
foreach($profile in @(@($true,$false,2048,32),@($true,$false,1024,32),@($true,$false,0,32),@($true,$true,4096,32),@($false,$false,4096,32),@($true,$false,4096,33))){
 $threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $profile[0] $profile[1] $profile[2] $profile[3]|Out-Null}catch{$threw=$true};Check $threw 'invalid32 config'
 Check (-not(Test-CaptureObservationCompletion $c32 $nonce '1234' '5678' '11' '22' $document @($page) $profile[0] $profile[1] $profile[2] $profile[3])) 'invalid32 completion'
 Check (-not(Test-CaptureOwnerRefusal $d32 $nonce '1234' '5678' '11' '22' $false $profile[0] $profile[1] $profile[2] $profile[3])) 'invalid32 refusal'
}
$ReceiverSubtreeItemCap=4096;$ReceiverSubtreeDepthCap=32;$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap,[bool]$allowUnfocused=$false) if($allowUnfocused -or $args.Count -or $focus -or -not $subtree -or $profile512 -or $itemCap -ne 4096 -or $depthCap -ne 32){throw 'Wrongdepth32 route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actualdepth32 collector route'
$operatorText=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-capture-observation-source.ps1'))
$guard=$operatorText.Substring($operatorText.IndexOf('if($ReceiverSubtreeItemCap -eq 4096'),$operatorText.IndexOf('if($ReceiverSubtreeItemCap -notin')-$operatorText.IndexOf('if($ReceiverSubtreeItemCap -eq 4096'))
& ([scriptblock]::Create($guard))
Check $true 'actualoperator selected32 guard'
$ReceiverSubtreeItemCap=2048;$rejected=$false;try{& ([scriptblock]::Create($guard))}catch{$rejected=$true};Check $rejected 'actualoperator32 only4096'
$ReceiverSubtreeItemCap=4096;$localBindings=@{}
$mapping=($operatorText -split "`n"|Where-Object {$_ -like 'if($ReceiverSubtreeCapture){$localBindings.discovery_scope=*'})
& ([scriptblock]::Create($mapping));Check ($localBindings.discovery_scope -ceq 'receiver-subtree-capture-unqualified-v7') 'actualoperator v7 binding'
$ReceiverSubtreeDepthCap=16;& ([scriptblock]::Create($mapping));Check ($localBindings.discovery_scope -ceq 'receiver-subtree-capture-unqualified-v6') 'actualoperator historicv6 binding'
Write-Output "PASS receiver subtree depth32 focused consumer $($checks-$start32) checks; historicalv6 preserved"
