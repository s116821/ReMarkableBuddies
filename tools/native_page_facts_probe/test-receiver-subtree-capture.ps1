$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-observation-proof.ps1"
. "$PSScriptRoot/capture-owner-refusal-proof.ps1"
. "$PSScriptRoot/capture-observation-build-config.ps1"
$nonce='0123456789abcdef0123456789abcdef';$document='11111111-1111-1111-1111-111111111111';$page='22222222-2222-2222-2222-222222222222'
$png=[byte[]]::new(70);$sha='a'*64;$checks=0
function Import-Function([string]$File,[string]$Name){
    $tokens=$null;$errors=$null;$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $File),[ref]$tokens,[ref]$errors)
    $fn=$ast.Find({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $Name},$true)
    if($errors.Count -or -not $fn){throw 'Fixture helper missing'}
    return [scriptblock]::Create($fn.Extent.Text)
}
. (Import-Function 'test-capture-observation-proof.ps1' 'New-Completion')
. (Import-Function 'test-capture-topology-version.ps1' 'New-V1')
. (Import-Function 'test-capture-topology-version.ps1' 'New-V2')
function Check([bool]$value,[string]$name){if(-not $value){throw "FAIL $name"};$script:checks++}
function Completion($v,[bool]$selected=$true){Test-CaptureObservationCompletion $v $nonce '1234' '5678' '11' '22' $document @($page) $selected}
function Refusal($v,[bool]$selected=$true){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $false $selected}
function New-V5 {$v=New-V2;$v.version=5;$v|Add-Member discovery_scope 'receiver-subtree-capture-unqualified-v1';return $v}
$c=New-Completion;$c.version=2;$c|Add-Member discovery_scope 'receiver-subtree-capture-unqualified-v1'
Check (Completion $c) 'scoped completion37'
Check (-not(Completion $c $false)) 'unselected scoped completion'
Check (Completion (New-Completion) $false) 'ordinary completion36 preserved'
Check (-not(Completion (New-Completion))) 'selected scope refuses ordinary completion'
foreach($case in @('scope','missing','extra','version','authority')){
    $bad=$c|ConvertTo-Json|ConvertFrom-Json
    switch($case){'scope'{$bad.discovery_scope='window-focus-ancestry-v1'};'missing'{$bad.PSObject.Properties.Remove('discovery_scope')};'extra'{$bad|Add-Member extra $null};'version'{$bad.version=1};'authority'{$bad.native_authority=$true}}
    Check (-not(Completion $bad)) "completion refuses $case"
}
Check (Refusal (New-V5)) 'initial scoped refusal34'
Check (-not(Refusal (New-V5) $false)) 'unselected scoped refusal'
$v=New-V5;$v.branch='owner-discovery';$v.predicate=$null;$v.discovery_result='open-owner-unavailable';$v.visited_items=4;$v.receiver_candidates=1;$v.scene_candidates=0;$v.matched_pairs=0
Check (Refusal $v) 'complete no scene'
$two=$v|ConvertTo-Json|ConvertFrom-Json;$two.discovery_result='open-owner-ambiguous';$two.scene_candidates=2;$two.matched_pairs=2
Check (Refusal $two) 'two valid scenes'
$bound=$v|ConvertTo-Json|ConvertFrom-Json;$bound.discovery_result='open-capture-subtree-bound';$bound.matched_pairs=$null;$bound.topology_limit='subtree-items';$bound.topology_depth=0;$bound.topology_queue_size=1;$bound.topology_child_count=257;$bound.visited_items=1
Check (Refusal $bound) 'subtree item cap'
$depth=$bound|ConvertTo-Json|ConvertFrom-Json;$depth.topology_limit='subtree-depth';$depth.topology_depth=8;$depth.topology_queue_size=12;$depth.topology_child_count=1;$depth.visited_items=9
Check (Refusal $depth) 'subtree depth cap'
foreach($case in @('scope','missing','extra','over256','receiver2','depth24','queue4096','old-label','wrong-limit','partial-counter','version')){
    $bad=$bound|ConvertTo-Json|ConvertFrom-Json
    switch($case){'scope'{$bad.discovery_scope='window-focus-ancestry-v1'};'missing'{$bad.PSObject.Properties.Remove('discovery_scope')};'extra'{$bad|Add-Member extra $null};'over256'{$bad.visited_items=257};'receiver2'{$bad.receiver_candidates=2};'depth24'{$bad.topology_depth=24};'queue4096'{$bad.topology_queue_size=4096};'old-label'{$bad.discovery_result='open-topology-bound'};'wrong-limit'{$bad.topology_limit='queue-cap'};'partial-counter'{$bad.scene_candidates=$null};'version'{$bad.version=2}}
    Check (-not(Refusal $bad)) "refusal rejects $case"
}
$expected=[pscustomobject]@{nonce=$nonce;document=$document;order=@(1..6|ForEach-Object {'00000000-0000-4000-8000-{0:x12}' -f $_})}
$config=Get-CaptureObservationBuildConfig $expected $false $true
Check ($config.Contains('config.developmentReceiverSubtreeCapture=true;') -and -not $config.Contains('config.developmentFocusAncestry=true;')) 'explicit config'
$threw=$false;try{Get-CaptureObservationBuildConfig $expected $true $true|Out-Null}catch{$threw=$true};Check $threw 'mutually exclusive config'
$tokens=$null;$errors=$null;$operator=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'run-qt-capture-observation-source.ps1'),[ref]$tokens,[ref]$errors)
Check (-not $errors.Count) 'operator source parses'
Check ($operator.EndBlock.Statements[1].Extent.Text -like "throw 'SOURCE ONLY:*") 'source-only guard preserved'
$call=@($operator.FindAll({param($node)$node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -ceq 'Receive-CaptureOwnerRefusalEvidence'},$true))[0]
$command=$call.Extent.Text
foreach($block in @($call.CommandElements|Where-Object {$_ -is [Management.Automation.Language.ScriptBlockExpressionAst]})){$command=$command.Replace($block.Extent.Text,'{}')}
$FocusAncestry=$false;$ReceiverSubtreeCapture=$true;$ReceiverSubtreeCapture512=$false;$ReceiverSubtreeItemCap=0;$remote='inert';$packet='inert';$ownerIdentity=$null;$record=@{};$script:routed=$false
& {function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$focus,[bool]$subtree,[bool]$profile512,[int]$itemCap=0) if($args.Count -or $focus -or -not $subtree -or $profile512 -or $itemCap){throw 'Wrong scope route'};$script:routed=$true}; & ([scriptblock]::Create($command))}
Check $script:routed 'actual owner collector route'
Write-Output "PASS receiver subtree scoped consumer $checks checks (synthetic wire/config; no native authority)"
