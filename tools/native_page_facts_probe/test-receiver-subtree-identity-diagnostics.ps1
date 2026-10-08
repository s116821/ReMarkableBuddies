$ErrorActionPreference='Stop'
. "$PSScriptRoot/test-receiver-subtree-unfocused-area.ps1"
$start9=$checks
function Completion9($v){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $true $false 4096 32 $true $true}
function Refusal9($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $true $false 4096 32 $true $true}
$c9=$c8|ConvertTo-Json|ConvertFrom-Json;$c9.discovery_scope='receiver-subtree-capture-unqualified-v9'
Check (Completion9 $c9) 'v9 completion exact37'
Check (-not(Completion8 $c9)) 'v8 refuses v9 completion'
Check (-not(Completion9 $c8)) 'v9 refuses v8 completion'
$r9=$v|ConvertTo-Json|ConvertFrom-Json;$r9.discovery_scope='receiver-subtree-capture-unqualified-v9';$r9.scene_candidates=1;$r9.first_pair_receiver=0;$r9.first_pair_scene=0;$r9.first_pair_rejection='identity-page-order-mismatch'
Check (Refusal9 $r9) 'v9 fixed comparison refusal exact34'
Check (-not(Refusal8 $r9)) 'v8 refuses v9 refusal'
$rOld=$r9|ConvertTo-Json|ConvertFrom-Json;$rOld.discovery_scope='receiver-subtree-capture-unqualified-v8';$rOld.first_pair_rejection='capture-identity'
Check (Refusal8 $rOld) 'historicalv8 fallback preserved'
Check (-not(Refusal9 $rOld)) 'v9 refuses v8 refusal'
foreach($reason in @('identity-document-type','identity-document-mismatch','identity-index-type','identity-index-range','identity-scene-page-type','identity-receiver-page-type','identity-page-alias-mismatch','identity-page-order-mismatch','identity-context')){
 $bad=$r9|ConvertTo-Json|ConvertFrom-Json;$bad.first_pair_rejection=$reason;Check (Refusal9 $bad) 'selected fixed reason allowlist'
 $bad.discovery_scope='receiver-subtree-capture-unqualified-v8';Check (-not(Refusal8 $bad)) 'historicalv8 rejects detailed reason'
}
$bad=$r9|ConvertTo-Json|ConvertFrom-Json;$bad.first_pair_rejection='identity-document-value';Check (-not(Refusal9 $bad)) 'unknown reason refused'
$config9=Get-CaptureObservationBuildConfig $expected $false $true $false 4096 32 $true $true
Check ($config9.Contains('config.developmentReceiverSubtreeCaptureDetailedIdentityDiagnostics=true;')) 'explicit v9 config'
Check (-not($config8.Contains('developmentReceiverSubtreeCaptureDetailedIdentityDiagnostics=true;'))) 'default v8 config unchanged'
$threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $true $false 4096 32 $false $true|Out-Null}catch{$threw=$true};Check $threw 'v9 requires v8 option config'
Check (-not(Test-CaptureObservationCompletion $c9 $nonce '1234' '5678' '11' '22' $document @($page) $true $false 4096 32 $false $true)) 'v9 requires v8 completion'
Check (-not(Test-CaptureOwnerRefusal $r9 $nonce '1234' '5678' '11' '22' $false $true $false 4096 32 $false $true)) 'v9 requires v8 refusal'
$ReceiverSubtreeCaptureDetailedIdentityDiagnostics=$true;$ReceiverSubtreeCaptureAllowUnfocusedArea=$true;$ReceiverSubtreeCapture=$true;$ReceiverSubtreeCapture512=$false;$ReceiverSubtreeItemCap=4096;$ReceiverSubtreeDepthCap=32;$FocusAncestry=$false;$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap,[bool]$allowUnfocused,[bool]$detailedIdentity,[bool]$documentType=$false) if($documentType -or $args.Count -or $focus -or -not $subtree -or $profile512 -or $itemCap -ne 4096 -or $depthCap -ne 32 -or -not $allowUnfocused -or -not $detailedIdentity){throw 'Wrongv9 refusal route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actualoperator v9 refusal route'
$operatorText=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-capture-observation-source.ps1'))
$guard=$operatorText.Substring($operatorText.IndexOf('if($ReceiverSubtreeCaptureDetailedIdentityDiagnostics -and'),$operatorText.IndexOf('if($ReceiverSubtreeItemCap -notin')-$operatorText.IndexOf('if($ReceiverSubtreeCaptureDetailedIdentityDiagnostics -and'))
& ([scriptblock]::Create($guard));Check $true 'actualoperator v9 guard'
$ReceiverSubtreeCaptureAllowUnfocusedArea=$false;$threw=$false;try{& ([scriptblock]::Create($guard))}catch{$threw=$true};Check $threw 'actualoperator v9 requires v8';$ReceiverSubtreeCaptureAllowUnfocusedArea=$true
$mapping=($operatorText -split "`n"|Where-Object {$_ -like 'if($ReceiverSubtreeCapture){$localBindings.discovery_scope=*'});$localBindings=@{}
& ([scriptblock]::Create($mapping));Check ($localBindings.discovery_scope -ceq 'receiver-subtree-capture-unqualified-v9') 'actualoperator v9 binding'
$ReceiverSubtreeCaptureDetailedIdentityDiagnostics=$false;& ([scriptblock]::Create($mapping));Check ($localBindings.discovery_scope -ceq 'receiver-subtree-capture-unqualified-v8') 'actualoperator defaultv8 binding'
$ReceiverSubtreeCaptureDetailedIdentityDiagnostics=$true;$script:completionRouted=$false
& {function Receive-CaptureObservation {param($a,$b,$c,$d,$e,$f,$read,$copy,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap,[bool]$allowUnfocused,[bool]$detailedIdentity,[bool]$documentType=$false) if($documentType -or $args.Count -or -not $subtree -or $profile512 -or $itemCap -ne 4096 -or $depthCap -ne 32 -or -not $allowUnfocused -or -not $detailedIdentity){throw 'Wrongv9 completion route'};$script:completionRouted=$true}; & ([scriptblock]::Create($completionCommand))}
Check $script:completionRouted 'actualoperator v9 completion route'
Write-Output "PASS receiver subtree identity diagnostics focused consumer $($checks-$start9) checks; historicalv1-v8 preserved"
