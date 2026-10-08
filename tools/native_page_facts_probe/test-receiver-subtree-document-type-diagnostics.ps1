$ErrorActionPreference='Stop'
. "$PSScriptRoot/test-receiver-subtree-identity-diagnostics.ps1"
$start10=$checks
function Completion10($v){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $true $false 4096 32 $true $true $true}
function Refusal10($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $true $false 4096 32 $true $true $true}
$c10=$c9|ConvertTo-Json|ConvertFrom-Json;$c10.discovery_scope='receiver-subtree-capture-unqualified-v10'
Check (Completion10 $c10) 'v10 completion exact37'
Check (-not(Completion9 $c10)) 'v9 refuses v10 completion'
Check (-not(Completion10 $c9)) 'v10 refuses v9 completion'
$r10=$r9|ConvertTo-Json|ConvertFrom-Json;$r10.discovery_scope='receiver-subtree-capture-unqualified-v10';$r10.first_pair_rejection='identity-document-type-quuid'
Check (Refusal10 $r10) 'v10 typed QUuid refusal exact34'
$other=$r10|ConvertTo-Json|ConvertFrom-Json;$other.first_pair_rejection='identity-document-type-other';Check (Refusal10 $other) 'v10 other type refusal'
Check (-not(Refusal9 $r10)) 'v9 refuses v10 refusal'
$invalid=$r10|ConvertTo-Json|ConvertFrom-Json;$invalid.first_pair_rejection='identity-document-type-invalid';Check (Refusal10 $invalid) 'v10 invalid QVariant refusal'
foreach($reason in @('identity-document-type-invalid','identity-document-type-quuid','identity-document-type-other')){
 $bad=$r10|ConvertTo-Json|ConvertFrom-Json;$bad.discovery_scope='receiver-subtree-capture-unqualified-v9';$bad.first_pair_rejection=$reason;Check (-not(Refusal9 $bad)) 'v9 rejects selected type labels'
}
$historic=$r9|ConvertTo-Json|ConvertFrom-Json;$historic.first_pair_rejection='identity-document-type';Check (Refusal9 $historic) 'historicalv9 generic type refusal'
$bad=$r10|ConvertTo-Json|ConvertFrom-Json;$bad.first_pair_rejection='identity-document-type-bytearray';Check (-not(Refusal10 $bad)) 'unknown bucket refused'
$config10=Get-CaptureObservationBuildConfig $expected $false $true $false 4096 32 $true $true $true
Check ($config10.Contains('config.developmentReceiverSubtreeCaptureDocumentIdTypeDiagnostics=true;')) 'explicitv10 config'
Check (-not($config9.Contains('developmentReceiverSubtreeCaptureDocumentIdTypeDiagnostics=true;'))) 'defaultv9 unchanged'
$threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $true $false 4096 32 $true $false $true|Out-Null}catch{$threw=$true};Check $threw 'v10 requires detailed config'
Check (-not(Test-CaptureObservationCompletion $c10 $nonce '1234' '5678' '11' '22' $document @($page) $true $false 4096 32 $true $false $true)) 'v10 requires detailed completion'
Check (-not(Test-CaptureOwnerRefusal $r10 $nonce '1234' '5678' '11' '22' $false $true $false 4096 32 $true $false $true)) 'v10 requires detailed refusal'
$ReceiverSubtreeCaptureDocumentIdTypeDiagnostics=$true;$ReceiverSubtreeCaptureDetailedIdentityDiagnostics=$true;$ReceiverSubtreeCaptureAllowUnfocusedArea=$true;$ReceiverSubtreeCapture=$true;$ReceiverSubtreeCapture512=$false;$ReceiverSubtreeItemCap=4096;$ReceiverSubtreeDepthCap=32;$FocusAncestry=$false;$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap,[bool]$allowUnfocused,[bool]$detailedIdentity,[bool]$documentType,[bool]$entryConversion=$false) if($entryConversion -or $args.Count -or $focus -or -not $subtree -or $profile512 -or $itemCap -ne 4096 -or $depthCap -ne 32 -or -not $allowUnfocused -or -not $detailedIdentity -or -not $documentType){throw 'Wrongv10 refusal route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actualoperator v10 refusal route'
$operatorText=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-capture-observation-source.ps1'))
$guard=$operatorText.Substring($operatorText.IndexOf('if($ReceiverSubtreeCaptureDocumentIdTypeDiagnostics -and'),$operatorText.IndexOf('if($ReceiverSubtreeItemCap -notin')-$operatorText.IndexOf('if($ReceiverSubtreeCaptureDocumentIdTypeDiagnostics -and'))
& ([scriptblock]::Create($guard));Check $true 'actualoperator v10 guard'
$ReceiverSubtreeCaptureDetailedIdentityDiagnostics=$false;$threw=$false;try{& ([scriptblock]::Create($guard))}catch{$threw=$true};Check $threw 'actualoperator v10 requires detailed';$ReceiverSubtreeCaptureDetailedIdentityDiagnostics=$true
$mapping=($operatorText -split "`n"|Where-Object {$_ -like 'if($ReceiverSubtreeCapture){$localBindings.discovery_scope=*'});$localBindings=@{}
& ([scriptblock]::Create($mapping));Check ($localBindings.discovery_scope -ceq 'receiver-subtree-capture-unqualified-v10') 'actualoperator v10 binding'
$ReceiverSubtreeCaptureDocumentIdTypeDiagnostics=$false;& ([scriptblock]::Create($mapping));Check ($localBindings.discovery_scope -ceq 'receiver-subtree-capture-unqualified-v9') 'actualoperator defaultv9 binding'
$ReceiverSubtreeCaptureDocumentIdTypeDiagnostics=$true;$script:completionRouted=$false
& {function Receive-CaptureObservation {param($a,$b,$c,$d,$e,$f,$read,$copy,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap,[bool]$allowUnfocused,[bool]$detailedIdentity,[bool]$documentType,[bool]$entryConversion=$false) if($entryConversion -or $args.Count -or -not $subtree -or $profile512 -or $itemCap -ne 4096 -or $depthCap -ne 32 -or -not $allowUnfocused -or -not $detailedIdentity -or -not $documentType){throw 'Wrongv10 completion route'};$script:completionRouted=$true}; & ([scriptblock]::Create($completionCommand))}
Check $script:completionRouted 'actualoperator v10 completion route'
Write-Output "PASS receiver subtree document type diagnostics focused consumer $($checks-$start10) checks; historicalv1-v9 preserved"
