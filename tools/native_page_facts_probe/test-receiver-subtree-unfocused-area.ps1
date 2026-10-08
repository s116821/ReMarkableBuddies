$ErrorActionPreference='Stop'
. "$PSScriptRoot/test-receiver-subtree-depth32.ps1"
$start8=$checks
function Completion8($v){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $true $false 4096 32 $true}
function Refusal8($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $true $false 4096 32 $true}
$c8=$c32|ConvertTo-Json|ConvertFrom-Json;$c8.discovery_scope='receiver-subtree-capture-unqualified-v8'
Check (Completion8 $c8) 'v8 completion'
Check (-not(Completion32 $c8)) 'v7 refuses v8 completion'
Check (-not(Completion8 $c32)) 'v8 refuses v7 completion'
$r8=$d32|ConvertTo-Json|ConvertFrom-Json;$r8.discovery_scope='receiver-subtree-capture-unqualified-v8'
Check (Refusal8 $r8) 'v8 exact refusal shape'
Check (-not(Refusal32 $r8)) 'v7 refuses v8 refusal'
Check (-not(Refusal8 $d32)) 'v8 refuses v7 refusal'
$config8=Get-CaptureObservationBuildConfig $expected $false $true $false 4096 32 $true
Check ($config8.Contains('config.developmentReceiverSubtreeCaptureAllowUnfocusedArea=true;')) 'explicitv8 config'
Check (-not($config32.Contains('config.developmentReceiverSubtreeCaptureAllowUnfocusedArea=true;'))) 'default v7 remains strict'
foreach($profile in @(@($false,$false,4096,32),@($true,$true,4096,32),@($true,$false,2048,32),@($true,$false,4096,16),@($true,$false,0,0))){
 $threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $profile[0] $profile[1] $profile[2] $profile[3] $true|Out-Null}catch{$threw=$true};Check $threw 'invalidv8 config'
 Check (-not(Test-CaptureObservationCompletion $c8 $nonce '1234' '5678' '11' '22' $document @($page) $profile[0] $profile[1] $profile[2] $profile[3] $true)) 'invalidv8 completion'
 Check (-not(Test-CaptureOwnerRefusal $r8 $nonce '1234' '5678' '11' '22' $false $profile[0] $profile[1] $profile[2] $profile[3] $true)) 'invalidv8 refusal'
}
$threw=$false;try{Get-CaptureObservationBuildConfig $expected $true $true $false 4096 32 $true|Out-Null}catch{$threw=$true};Check $threw 'v8 rejects focus selection config'
Check (-not(Test-CaptureOwnerRefusal $r8 $nonce '1234' '5678' '11' '22' $true $true $false 4096 32 $true)) 'v8 rejects focus selection refusal'
$ReceiverSubtreeCaptureAllowUnfocusedArea=$true;$ReceiverSubtreeCapture=$true;$ReceiverSubtreeCapture512=$false;$ReceiverSubtreeItemCap=4096;$ReceiverSubtreeDepthCap=32;$FocusAncestry=$false;$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap,[bool]$allowUnfocused,[bool]$detailedIdentity=$false) if($detailedIdentity -or $args.Count -or $focus -or -not $subtree -or $profile512 -or $itemCap -ne 4096 -or $depthCap -ne 32 -or -not $allowUnfocused){throw 'Wrongv8 refusal route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actualoperator v8 refusal route'
$operatorText=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-capture-observation-source.ps1'))
$guard=$operatorText.Substring($operatorText.IndexOf('if($ReceiverSubtreeCaptureAllowUnfocusedArea -and'),$operatorText.IndexOf('if($ReceiverSubtreeItemCap -notin')-$operatorText.IndexOf('if($ReceiverSubtreeCaptureAllowUnfocusedArea -and'))
& ([scriptblock]::Create($guard));Check $true 'actualoperator v8 guard'
$FocusAncestry=$true;$threw=$false;try{& ([scriptblock]::Create($guard))}catch{$threw=$true};Check $threw 'actualoperator v8 rejects focus';$FocusAncestry=$false
$mapping=($operatorText -split "`n"|Where-Object {$_ -like 'if($ReceiverSubtreeCapture){$localBindings.discovery_scope=*'});$localBindings=@{}
& ([scriptblock]::Create($mapping));Check ($localBindings.discovery_scope -ceq 'receiver-subtree-capture-unqualified-v8') 'actualoperator v8 binding'
$ReceiverSubtreeCaptureAllowUnfocusedArea=$false;& ([scriptblock]::Create($mapping));Check ($localBindings.discovery_scope -ceq 'receiver-subtree-capture-unqualified-v7') 'actualoperator defaultv7 binding'
$ReceiverSubtreeCaptureAllowUnfocusedArea=$true;$script:completionRouted=$false
$completionCall=$operator.FindAll({param($node)$node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -ceq 'Receive-CaptureObservation'},$true)|Select-Object -First 1
$completionCommand=$completionCall.Extent.Text
foreach($block in @($completionCall.CommandElements|Where-Object {$_ -is [Management.Automation.Language.ScriptBlockExpressionAst]})){$completionCommand=$completionCommand.Replace($block.Extent.Text,'{}')}
& {function Receive-CaptureObservation {param($a,$b,$c,$d,$e,$f,$read,$copy,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap,[bool]$allowUnfocused,[bool]$detailedIdentity=$false) if($detailedIdentity -or $args.Count -or -not $subtree -or $profile512 -or $itemCap -ne 4096 -or $depthCap -ne 32 -or -not $allowUnfocused){throw 'Wrongv8 completion route'};$script:completionRouted=$true}; & ([scriptblock]::Create($completionCommand))}
Check $script:completionRouted 'actualoperator v8 completion route'
Write-Output "PASS receiver subtree unfocused-area focused consumer $($checks-$start8) checks; historicalv1-v7 preserved"
