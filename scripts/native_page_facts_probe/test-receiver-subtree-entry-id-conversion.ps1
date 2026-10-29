$ErrorActionPreference='Stop'
. "$PSScriptRoot/test-receiver-subtree-document-type-diagnostics.ps1"
$start11=$checks
function Completion11($v){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $true $false 4096 32 $true $true $true $true}
function Refusal11($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $true $false 4096 32 $true $true $true $true}
$c11=$c10|ConvertTo-Json|ConvertFrom-Json;$c11.discovery_scope='receiver-subtree-capture-unqualified-v11'
Check (Completion11 $c11) 'v11 completion exact37'
Check (-not(Completion10 $c11)) 'v10 refuses v11 completion'
Check (-not(Completion11 $c10)) 'v11 refuses v10 completion'
$r11=$r10|ConvertTo-Json|ConvertFrom-Json;$r11.discovery_scope='receiver-subtree-capture-unqualified-v11'
foreach($reason in @('identity-entry-type-unavailable','identity-entry-type-mismatch','identity-entry-converter-unavailable','identity-entry-conversion-failed')){
 $r11.first_pair_rejection=$reason;Check (Refusal11 $r11) 'v11 fixed entry refusal exact34'
 $old=$r11|ConvertTo-Json|ConvertFrom-Json;$old.discovery_scope='receiver-subtree-capture-unqualified-v10';Check (-not(Refusal10 $old)) 'v10 refuses entry reason'
}
$r11.first_pair_rejection='identity-context';Check (Refusal11 $r11) 'v11 context guard refusal'
$r11.first_pair_rejection='identity-entry-unknown';Check (-not(Refusal11 $r11)) 'unknown entry reason refused'
$config11=Get-CaptureObservationBuildConfig $expected $false $true $false 4096 32 $true $true $true $true
Check ($config11.Contains('config.developmentReceiverSubtreeCaptureEntryIdConversion=true;')) 'explicitv11 config'
Check (-not($config10.Contains('developmentReceiverSubtreeCaptureEntryIdConversion=true;'))) 'defaultv10 unchanged'
$threw=$false;try{Get-CaptureObservationBuildConfig $expected $false $true $false 4096 32 $true $true $false $true|Out-Null}catch{$threw=$true};Check $threw 'v11 requires document type config'
Check (-not(Test-CaptureObservationCompletion $c11 $nonce '1234' '5678' '11' '22' $document @($page) $true $false 4096 32 $true $true $false $true)) 'v11 requires document type completion'
Check (-not(Test-CaptureOwnerRefusal $r11 $nonce '1234' '5678' '11' '22' $false $true $false 4096 32 $true $true $false $true)) 'v11 requires document type refusal'
$ReceiverSubtreeCaptureEntryIdConversion=$true;$ReceiverSubtreeCaptureDocumentIdTypeDiagnostics=$true;$ReceiverSubtreeCaptureDetailedIdentityDiagnostics=$true;$ReceiverSubtreeCaptureAllowUnfocusedArea=$true
$guard11=$operatorText.Substring($operatorText.IndexOf('if($ReceiverSubtreeCaptureEntryIdConversion -and'),$operatorText.IndexOf('if($ReceiverSubtreeItemCap -notin')-$operatorText.IndexOf('if($ReceiverSubtreeCaptureEntryIdConversion -and'))
& ([scriptblock]::Create($guard11));Check $true 'actualoperator v11 guard'
$ReceiverSubtreeCaptureDocumentIdTypeDiagnostics=$false;$threw=$false;try{& ([scriptblock]::Create($guard11))}catch{$threw=$true};Check $threw 'actualoperator v11 requires document type';$ReceiverSubtreeCaptureDocumentIdTypeDiagnostics=$true
& ([scriptblock]::Create($mapping));Check ($localBindings.discovery_scope -ceq 'receiver-subtree-capture-unqualified-v11') 'actualoperator v11 binding'
$ReceiverSubtreeCaptureEntryIdConversion=$false;& ([scriptblock]::Create($mapping));Check ($localBindings.discovery_scope -ceq 'receiver-subtree-capture-unqualified-v10') 'actualoperator defaultv10 binding'
$ReceiverSubtreeCaptureEntryIdConversion=$true;$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap,[bool]$allowUnfocused,[bool]$detailedIdentity,[bool]$documentType,[bool]$entryConversion) if($args.Count -or $focus -or -not $subtree -or $profile512 -or $itemCap -ne 4096 -or $depthCap -ne 32 -or -not $allowUnfocused -or -not $detailedIdentity -or -not $documentType -or -not $entryConversion){throw 'Wrongv11 refusal route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actualoperator v11 refusal route'
$script:completionRouted=$false
& {function Receive-CaptureObservation {param($a,$b,$c,$d,$e,$f,$read,$copy,[bool]$subtree,[bool]$profile512,[int]$itemCap,[int]$depthCap,[bool]$allowUnfocused,[bool]$detailedIdentity,[bool]$documentType,[bool]$entryConversion) if($args.Count -or -not $subtree -or $profile512 -or $itemCap -ne 4096 -or $depthCap -ne 32 -or -not $allowUnfocused -or -not $detailedIdentity -or -not $documentType -or -not $entryConversion){throw 'Wrongv11 completion route'};$script:completionRouted=$true}; & ([scriptblock]::Create($completionCommand))}
Check $script:completionRouted 'actualoperator v11 completion route'
Write-Output "PASS receiver subtree entry ID conversion focused consumer $($checks-$start11) checks; historicalv1-v10 preserved"
