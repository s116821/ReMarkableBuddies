$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-owner-refusal-proof.ps1"
. "$PSScriptRoot/capture-owner-refusal-collector.ps1"
. "$PSScriptRoot/facts-refusal-proof.ps1"
$nonce='0123456789abcdef0123456789abcdef';$checks=0
function Import-FixtureFunction([string]$File,[string]$Name){
    $tokens=$null;$errors=$null;$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $File),[ref]$tokens,[ref]$errors)
    $fn=$ast.Find({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $Name},$true)
    if($errors.Count -or -not $fn){throw 'Fixture helper missing'}
    return [scriptblock]::Create($fn.Extent.Text)
}
. (Import-FixtureFunction 'test-capture-topology-version.ps1' 'New-V1')
. (Import-FixtureFunction 'test-capture-topology-version.ps1' 'New-V2')
. (Import-FixtureFunction 'test-facts-refusal.ps1' 'New-Refusal')
function Check([bool]$Value,[string]$Name){if(-not $Value){throw "FAIL $Name"};$script:checks++}
function New-V3 {
    $v=New-V2;$v.version=3
    $v|Add-Member discovery_scope 'window-focus-ancestry-v1'
    $v|Add-Member chain_items $null;$v|Add-Member chain_complete $false;$v|Add-Member chain_failure $null
    return $v
}
function Valid($v,[bool]$Selected=$true){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22' $Selected}
function Local([string]$Label,[int]$Count){$v=New-V3;$v.branch='owner-discovery';$v.predicate=$null;$v.discovery_result='open-focus-chain-refused';$v.chain_failure=$Label;$v.chain_items=$Count;return $v}
Check (Valid (New-V3)) 'initial-progress37'
Check (-not(Valid (New-V3) $false)) 'unselected v3 refuses'
foreach($pair in @(@('anchor-unavailable',0),@('item-context',0),@('item-context',25),@('root-unreached',1),@('root-unreached',25),@('depth-bound',25),@('cycle',1),@('observer-unavailable',0),@('observer-unavailable',25))){Check (Valid (Local $pair[0] $pair[1])) "local $pair"}
foreach($pair in @(@('anchor-unavailable',1),@('root-unreached',0),@('depth-bound',24),@('cycle',0))){Check (-not(Valid (Local $pair[0] $pair[1]))) "bad local $pair"}
foreach($n in @(0,25)){$v=New-V3;$v.branch='owner-discovery';$v.discovery_result='open-context-lost';$v.chain_items=$n;Check (Valid $v) "progress count$n"}
$v=New-V3;$v.branch='owner-discovery';$v.discovery_result='open-context-lost';$v.chain_items=3;$v.chain_complete=$true;$v.visited_items=0;$v.receiver_candidates=0;$v.scene_candidates=0
Check (Valid $v) 'completed initial classification0'
foreach($name in @($v.PSObject.Properties.Name)){$bad=$v|ConvertTo-Json|ConvertFrom-Json;$bad.PSObject.Properties.Remove($name);Check (-not(Valid $bad)) "missing37 $name"}
foreach($field in @('chain_items','visited_items','receiver_candidates','scene_candidates','matched_pairs')){
    foreach($badValue in @($true,'3',1.5,-1,[bigint]::Parse('9223372036854775808'))){$bad=$v|ConvertTo-Json|ConvertFrom-Json;$bad.$field=$badValue;Check (-not(Valid $bad)) "bad scalar $field"}
}
foreach($case in @('scope','bool','complete-zero','over25','classification-over','matching-early','topology','failure-complete','unknown-label','progress-label','partial-counter','bfs-result','extra')){
    $bad=$v|ConvertTo-Json|ConvertFrom-Json
    switch($case){
        'scope'{$bad.discovery_scope='window-focus-ancestry-v2'};'bool'{$bad.chain_complete=1};'complete-zero'{$bad.chain_items=0};'over25'{$bad.chain_items=26}
        'classification-over'{$bad.visited_items=4};'matching-early'{$bad.matched_pairs=0};'topology'{$bad.topology_depth=1};'failure-complete'{$bad.chain_failure='item-context'}
        'unknown-label'{$bad=Local 'unknown' 1};'progress-label'{$bad=Local 'cycle' 1;$bad.predicate='invalidated'}
        'partial-counter'{$bad=Local 'cycle' 1;$bad.visited_items=0};'bfs-result'{$bad.discovery_result='open-topology-bound'};'extra'{$bad|Add-Member extra $null}
    }
    Check (-not(Valid $bad)) "matrix contradiction $case"
}
$raw=(New-V3)|ConvertTo-Json -Compress
Check ($null -eq (ConvertFrom-CaptureOwnerRefusalRaw $raw.Replace('"version":3','"version":3,"\u0076ersion":3'))) 'escaped duplicate'
$v=New-V3;$v.branch='observer-install';$v.predicate=$null;$v.discovery_result='open-owner-observed';$v.chain_items=3;$v.chain_complete=$true;$v.observer_role='document';$v.observer_member='pageMapChanged()';$v.observer_failure='connect-failed';Check (Valid $v) 'observer tuple'
$v=New-V3;$v.branch='owner-revalidation';$v.predicate='invalidated-before';$v.discovery_result='open-owner-observed';$v.chain_items=25;$v.chain_complete=$true;Check (Valid $v) 'revalidation tuple'
foreach($stage in @('open-owner-unavailable','open-owner-ambiguous','open-candidate-bound')){
    $v=New-V3;$v.branch='owner-discovery';$v.predicate=$null;$v.discovery_result=$stage;$v.chain_items=25;$v.chain_complete=$true;$v.visited_items=25;$v.receiver_candidates=0;$v.scene_candidates=0
    switch($stage){'open-owner-unavailable'{$v.matched_pairs=0};'open-owner-ambiguous'{$v.receiver_candidates=2;$v.scene_candidates=1;$v.matched_pairs=2};'open-candidate-bound'{$v.receiver_candidates=9}}
    Check (Valid $v) "classification $stage"
}
$r=New-Refusal;$r.reader_stage='facts-retained-owner-refused'
function Refusal($value,[bool]$Selected=$true){ConvertFrom-FactsRefusal ($value|ConvertTo-Json -Compress) $nonce 42 '1234' $Selected}
Check ($null -ne (Refusal $r)) 'selected reader-stage tuple'
Check ($null -eq (Refusal $r $false)) 'unselected reader-stage refuses'
foreach($case in @('had-facts','delivery','path','outer-stage')){
    $bad=$r|ConvertTo-Json|ConvertFrom-Json
    switch($case){'had-facts'{$bad.reader_result_had_facts=$true;$bad.refusal_path='entry-read-boundary'};'delivery'{$bad.reader_result_had_facts=$true;$bad.entry_stage='facts-entry-delivery-refused';$bad.refusal_path='entry-completion-boundary'};'path'{$bad.refusal_path='entry-completion-boundary'};'outer-stage'{$bad.entry_stage='facts-retained-owner-refused'}}
    Check ($null -eq (Refusal $bad)) "reader-stage contradiction $case"
}
$tmp=Join-Path ([IO.Path]::GetTempPath()) ('focus-proof-'+[guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $tmp)
$allocator=(Get-Command New-CaptureHistoricalTransportPath).ScriptBlock
function New-CaptureHistoricalTransportPath {& $allocator $tmp}
try{
    foreach($case in @('selected','unselected','unknown','later-loss')){
        $dir=Join-Path $tmp $case;[void](New-Item -ItemType Directory -Path $dir)
        $v=Local 'depth-bound' 25;if($case -eq 'unknown'){$v.version=4}
        $bytes=[Text.Encoding]::UTF8.GetBytes(($v|ConvertTo-Json -Compress));$hash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
        $record=@{};$script:reads=0
        $read={param($command)$script:reads++;[pscustomobject]@{exit=0;timeout=$false;stdout=$(if($case -eq 'later-loss' -and $script:reads -eq 2){"absent`n"}else{"present $($bytes.Length) $hash 33 44`n"})}}
        $copy={param($remote,$local)[IO.File]::WriteAllBytes($local,$bytes);[pscustomobject]@{exit=0;timeout=$false}}
        $failed=$false;try{Receive-CaptureOwnerRefusalEvidence ('/run/rmb-qt-probe-'+$nonce) $nonce ([pscustomobject]@{attempt_pid='1234';attempt_start='5678';root_device='11';root_inode='22'}) $dir $record $read $copy ($case -ne 'unselected')}catch{$failed=$true}
        Check ($record.capture_owner_refusal_saved_copy_verified -and (Get-FileHash "$dir/capture-owner-refusal.json").Hash.ToLowerInvariant() -ceq $hash) "raw preservation $case"
        Check ($record.capture_owner_refusal_decoded -eq ($case -eq 'selected')) "actual collector selection $case"
        Check ($failed -eq ($case -eq 'later-loss')) "binding failure $case"
    }
}finally{
    $absolute=[IO.Path]::GetFullPath($tmp);if(-not $absolute.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()),[StringComparison]::OrdinalIgnoreCase)){throw 'Fixture cleanup boundary'}
    Remove-Item -LiteralPath $absolute -Recurse -Force
}
# Extract only the actual operator command AST; transport callbacks are replaced
# with inert mocks. Never execute the source-only operator or its SSH/SCP bodies.
$tokens=$null;$errors=$null
$operator=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'run-qt-capture-observation-source.ps1'),[ref]$tokens,[ref]$errors)
Check ($errors.Count -eq 0) 'operator parses'
Check ($operator.EndBlock.Statements[1].Extent.Text -like "throw 'SOURCE ONLY:*") 'operator source-only guard remains'
foreach($name in @('Receive-CaptureRequestHistory','Receive-CaptureOwnerRefusalEvidence')){
    $calls=@($operator.FindAll({param($n)$n -is [Management.Automation.Language.CommandAst] -and $n.GetCommandName() -ceq $name},$true))
    Check ($calls.Count -eq 1) "operator single call $name"
    $call=$calls[0];$expected=if($name -eq 'Receive-CaptureOwnerRefusalEvidence'){13}else{8}
    Check ($call.CommandElements.Count -eq $expected) "operator arity $name"
    $command=$call.Extent.Text
    foreach($block in @($call.CommandElements | Where-Object {$_ -is [Management.Automation.Language.ScriptBlockExpressionAst]})){$command=$command.Replace($block.Extent.Text,'{}')}
    foreach($selected in @($false,$true)){
        $FocusAncestry=$selected;$remote='inert';$packet='inert';$ownerIdentity=$null;$record=@{}
        $script:routed=$null
        & {
            function Receive-CaptureRequestHistory {param($a,$b,$c,$d,$e,$read,$copy) if($args.Count){throw 'Extra history argument'};$script:routed=@{name='history';selected=$null}}
            function Receive-CaptureOwnerRefusalEvidence {param($a,$b,$c,$d,$e,$read,$copy,[bool]$FocusAncestry=$false,[bool]$ReceiverSubtreeCapture=$false,[bool]$ReceiverSubtreeCapture512=$false,[int]$ReceiverSubtreeItemCap=0,[int]$ReceiverSubtreeDepthCap=0) if($ReceiverSubtreeDepthCap -or $args.Count -or $ReceiverSubtreeCapture -or $ReceiverSubtreeCapture512 -or $ReceiverSubtreeItemCap){throw 'Extra owner argument or wrong scope'};$script:routed=@{name='owner';selected=$FocusAncestry}}
            & ([scriptblock]::Create($command))
        }
        Check ($null -ne $script:routed) "actual operator mock called $name/$selected"
        if($name -eq 'Receive-CaptureOwnerRefusalEvidence'){Check ($script:routed.selected -eq $selected) "actual operator owner selection $selected"}else{Check ($null -eq $script:routed.selected) "actual operator history no selection $selected"}
    }
}
Write-Output "PASS focus version3/refusal/actual mocked collector/operator $checks checks"
