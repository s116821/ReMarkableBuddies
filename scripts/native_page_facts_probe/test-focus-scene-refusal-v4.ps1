$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-owner-refusal-proof.ps1"
$nonce='0123456789abcdef0123456789abcdef';$checks=0
function Import-Function([string]$File,[string]$Name){
    $tokens=$null;$errors=$null
    $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $File),[ref]$tokens,[ref]$errors)
    $fn=$ast.Find({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $Name},$true)
    if($errors.Count -or -not $fn){throw 'Fixture helper missing'}
    return [scriptblock]::Create($fn.Extent.Text)
}
. (Import-Function 'test-capture-topology-version.ps1' 'New-V1')
. (Import-Function 'test-capture-topology-version.ps1' 'New-V2')
. (Import-Function 'test-focus-ancestry-version.ps1' 'New-V3')
$sceneFields=@('scene_rejected_engine','scene_rejected_class','scene_rejected_page_id','scene_rejected_page_id_changed','scene_rejected_document_wrapper_changed','scene_passed')
function New-V4 {
    $v=New-V3;$v.version=4
    foreach($field in $sceneFields){$v|Add-Member $field $null}
    return $v
}
function Valid($v,[bool]$Selected=$true){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $Selected}
function Check([bool]$Value,[string]$Name){if(-not $Value){throw "FAIL $Name"};$script:checks++}
Check (Valid (New-V4)) 'initial null43'
Check (-not (Valid (New-V4) $false)) 'unselected v4'
$v=New-V4;$v.branch='owner-discovery';$v.predicate=$null;$v.discovery_result='open-owner-unavailable'
$v.chain_complete=$true;$v.chain_items=8;$v.visited_items=8;$v.receiver_candidates=1;$v.scene_candidates=0;$v.matched_pairs=0
foreach($field in $sceneFields){$v.$field=0};$v.scene_rejected_class=8
Check (Valid $v) 'complete class rejection'
$zero=$v|ConvertTo-Json|ConvertFrom-Json;$zero.discovery_result='open-context-lost';$zero.predicate='invalidated';$zero.visited_items=0;$zero.receiver_candidates=0;$zero.matched_pairs=$null;$zero.scene_rejected_class=0
Check (Valid $zero) 'classified zero'
$partial=$v|ConvertTo-Json|ConvertFrom-Json;$partial.discovery_result='open-candidate-bound';$partial.chain_items=12;$partial.visited_items=11;$partial.scene_candidates=9;$partial.scene_passed=9;$partial.scene_rejected_class=2;$partial.matched_pairs=$null
Check (Valid $partial) 'partial candidate bound'
foreach($field in $sceneFields){$bad=$v|ConvertTo-Json|ConvertFrom-Json;$bad.PSObject.Properties.Remove($field);Check (-not(Valid $bad)) "missing $field"}
foreach($case in @('extra','bool','string','fraction','negative','over25','partial-null','sum','pass','old-version','preclassification','non-discovery')){
    $bad=$v|ConvertTo-Json|ConvertFrom-Json
    switch($case){
        'extra'{$bad|Add-Member extra $null};'bool'{$bad.scene_rejected_engine=$true};'string'{$bad.scene_rejected_engine='0'}
        'fraction'{$bad.scene_rejected_engine=0.5};'negative'{$bad.scene_rejected_engine=-1};'over25'{$bad.scene_rejected_engine=26}
        'partial-null'{$bad.scene_rejected_engine=$null};'sum'{$bad.scene_rejected_class=7};'pass'{$bad.scene_passed=1;$bad.scene_rejected_class=7}
        'old-version'{$bad.version=3};'preclassification'{$bad=New-V4;$bad.scene_rejected_engine=0}
        'non-discovery'{$bad=New-V4;$bad.branch='owner-revalidation';$bad.predicate='invalidated-before';$bad.discovery_result='open-owner-observed';$bad.chain_complete=$true;$bad.chain_items=8;$bad.scene_passed=0}
    }
    Check (-not(Valid $bad)) "refuse $case"
}
$local=New-V4;$local.branch='owner-discovery';$local.predicate=$null;$local.discovery_result='open-focus-chain-refused';$local.chain_items=25;$local.chain_failure='depth-bound'
Check (Valid $local) 'preclassification null'
$later=New-V4;$later.branch='owner-revalidation';$later.predicate='invalidated-before';$later.discovery_result='open-owner-observed';$later.chain_complete=$true;$later.chain_items=8
Check (Valid $later) 'later refusal null'
foreach($version in @(1,2,3)){$old= & (Get-Command "New-V$version");Check (Valid $old) "historical $version"}
Write-Output "PASS scene refusal v4 $checks checks"
