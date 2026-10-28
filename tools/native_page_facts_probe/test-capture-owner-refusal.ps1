$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-owner-refusal-proof.ps1"
. "$PSScriptRoot/capture-owner-refusal-collector.ps1"
. "$PSScriptRoot/capture-observation-collector.ps1"
$nonce='0123456789abcdef0123456789abcdef';$count=0
function Check([bool]$ok,[string]$name){if(-not $ok){throw "FAIL $name"};$script:count++}
function New-Refusal {
    [pscustomobject][ordered]@{kind='development-capture-owner-refusal';version=1;nonce=$nonce;attempt_pid='1234';attempt_start='5678';root_device='11';root_inode='22';setup_profile='main-dev-facts-120s';capture_accepted_ms=100;failure_ms=101;deadline_check_ms=$null;effective_deadline_ms=5100;branch='initial-progress';predicate='invalidated';discovery_result=$null;visited_items=$null;receiver_candidates=$null;scene_candidates=$null;matched_pairs=$null;first_pair_receiver=$null;first_pair_scene=$null;first_pair_rejection=$null;active_owner_rejection=$null;observer_role=$null;observer_member=$null;observer_failure=$null;native_authority=$false;render_authority=$false;ui_acknowledged=$false}
}
function Valid($value){Test-CaptureOwnerRefusal $value $nonce '1234' '5678' '11' '22'}
$good=New-Refusal;Check (Valid $good) 'valid initial'
foreach($field in @($good.PSObject.Properties.Name)){
    $bad=New-Refusal;$bad.PSObject.Properties.Remove($field);Check (-not(Valid $bad)) "missing $field"
}
foreach($field in @('attempt_pid','attempt_start','root_device','root_inode')){
    foreach($wrong in @('0','01','18446744073709551616',1)){$bad=New-Refusal;$bad.$field=$wrong;Check (-not(Valid $bad)) "identity $field $wrong"}
}
foreach($field in @('version','capture_accepted_ms','failure_ms','effective_deadline_ms')){
    foreach($wrong in @('1',1.5,$true,-1)){$bad=New-Refusal;$bad.$field=$wrong;Check (-not(Valid $bad)) "integer $field"}
}
foreach($field in @('native_authority','render_authority','ui_acknowledged')){$bad=New-Refusal;$bad.$field=$true;Check (-not(Valid $bad)) "authority $field"}
$raw=$good|ConvertTo-Json -Compress
Check (Valid (ConvertFrom-CaptureOwnerRefusalRaw $raw)) 'raw valid'
foreach($key in @('version','vers\u0069on')){Check ($null -eq (ConvertFrom-CaptureOwnerRefusalRaw $raw.Insert(1,('"'+$key+'":1,')))) 'raw duplicate refused'}
Check ($null -eq (ConvertFrom-CaptureOwnerRefusalRaw (' '*8193))) 'raw byte cap'
$bad=New-Refusal;$bad.predicate='deadline';$bad.deadline_check_ms=5100;$bad.failure_ms=9999;Check (Valid $bad) 'expired historical latch valid'
$bad.deadline_check_ms=5099;Check (-not(Valid $bad)) 'deadline operand must fail'
$bad=New-Refusal;$bad.branch='owner-discovery';$bad.predicate=$null;$bad.discovery_result='open-owner-unavailable';$bad.visited_items=3;$bad.receiver_candidates=1;$bad.scene_candidates=1;$bad.matched_pairs=0;$bad.first_pair_receiver=0;$bad.first_pair_scene=0;$bad.first_pair_rejection='drawing-area-focused';Check (Valid $bad) 'discovery first pair'
$bad.first_pair_scene=1;Check (-not(Valid $bad)) 'pair ordinal bounded by actual count'
$bad=New-Refusal;$bad.branch='observer-install';$bad.predicate=$null;$bad.discovery_result='open-owner-observed';$bad.observer_role='document';$bad.observer_member='pageMapChanged()';$bad.observer_failure='signal-missing';Check (Valid $bad) 'fixed observer tuple'
$bad.observer_member='runtimeName()';Check (-not(Valid $bad)) 'arbitrary member refused'
$bad=New-Refusal;$bad.branch='owner-revalidation';$bad.predicate='active-owner';$bad.discovery_result='open-owner-observed';$bad.active_owner_rejection='drawing-area-focused';Check (Valid $bad) 'revalidation group'
$bad.active_owner_rejection=$null;Check (-not(Valid $bad)) 'revalidation missing group'
foreach($predicate in @('context','live','invalidated','token','deadline')){
    $value=New-Refusal;$value.predicate=$predicate;if($predicate -ceq 'deadline'){$value.failure_ms=5100;$value.deadline_check_ms=5100}
    Check (Valid $value) "progress registry $predicate"
}
foreach($predicate in @('reentrant-check','invalidated-before','context-before','lifetime-before','active-owner','invalidated-after','context-after','lifetime-after','owner-pointers','owner-threads','deadline')){
    $value=New-Refusal;$value.branch='owner-revalidation';$value.discovery_result='open-owner-observed';$value.predicate=$predicate
    if($predicate -ceq 'active-owner'){$value.active_owner_rejection='document-identity'}
    if($predicate -ceq 'deadline'){$value.failure_ms=5100;$value.deadline_check_ms=5100}
    Check (Valid $value) "allowed registry $predicate"
}
foreach($group in @('pointers-or-threads','window-focus-active-visible','engine-association','window-association','item-visible-enabled','scene-active-focus','receiver-ancestor','drawing-area-focused','receiver-document','scene-document','document-identity')){
    $value=New-Refusal;$value.branch='owner-revalidation';$value.discovery_result='open-owner-observed';$value.predicate='active-owner';$value.active_owner_rejection=$group
    Check (Valid $value) "active registry $group"
}
foreach($result in @('open-engine-thread','open-current-window-unavailable','open-context-lost','open-item-lost','open-topology-bound','open-candidate-bound','open-owner-ambiguous','open-owner-unavailable')){
    $value=New-Refusal;$value.branch='owner-discovery';$value.predicate=$null;$value.discovery_result=$result
    if($result -cin @('open-item-lost','open-topology-bound','open-candidate-bound')){$value.visited_items=10;$value.receiver_candidates=$(if($result -ceq 'open-candidate-bound'){9}else{1});$value.scene_candidates=1}
    if($result -cin @('open-owner-ambiguous','open-owner-unavailable')){$value.visited_items=4;$value.receiver_candidates=2;$value.scene_candidates=1;$value.matched_pairs=$(if($result -ceq 'open-owner-ambiguous'){2}else{0})}
    if($result -ceq 'open-owner-unavailable'){$value.first_pair_receiver=0;$value.first_pair_scene=0;$value.first_pair_rejection='receiver-document'}
    Check (Valid $value) "discovery registry $result"
}
$members=@{document=@('pageCountChanged(int,int)','pageMapChanged()','pageAdded(int)','pagesAdded(QList<int>)','pageMoved(int,int)','pagesMoved()','pagesRemoved()','redirectionPageMapChanged()','pageUpdated(int)','documentMetadataChanged()','orientationChanged()');scene=@('pageIdChanged()','documentWrapperChanged()','workerChanged()','viewportChanged()');receiver=@('document','currentPage','currentPageId','drawingAreaFocused')}
foreach($result in @('open-owner-unavailable','open-owner-ambiguous','open-context-lost')){
 foreach($overflow in @('visited_items','receiver_candidates','scene_candidates')){
    $value=New-Refusal;$value.branch='owner-discovery';$value.predicate=$null;$value.discovery_result=$result;$value.visited_items=4;$value.receiver_candidates=2;$value.scene_candidates=1;$value.matched_pairs=$(if($result -ceq 'open-owner-ambiguous'){2}elseif($result -ceq 'open-context-lost'){1}else{0})
    if($result -ceq 'open-owner-unavailable'){$value.first_pair_receiver=0;$value.first_pair_scene=0;$value.first_pair_rejection='receiver-document'}
    $value.$overflow=$(if($overflow -ceq 'visited_items'){4097}else{9})
    Check (-not(Valid $value)) "post traversal overflow refused $result $overflow"
 }
}
$value=New-Refusal;$value.branch='owner-discovery';$value.predicate=$null;$value.discovery_result='open-owner-ambiguous';$value.visited_items=3;$value.receiver_candidates=1;$value.scene_candidates=1;$value.matched_pairs=2
Check (-not(Valid $value)) 'ambiguity requires two candidate pairs'
$value.discovery_result='open-context-lost';Check (-not(Valid $value)) 'context cannot follow immediate ambiguity'
$value.matched_pairs=0;$value.active_owner_rejection='document-identity';Check (-not(Valid $value)) 'final active rejection requires selected match'
$value.active_owner_rejection=$null;$value.receiver_candidates=0;Check (-not(Valid $value)) 'pair progress requires both roles'
foreach($role in $members.Keys){foreach($member in $members[$role]){foreach($failure in @('object-missing','property-missing','notify-missing','signal-missing','slot-missing','return-type','connect-failed')){
    $value=New-Refusal;$value.branch='observer-install';$value.predicate=$null;$value.discovery_result='open-owner-observed';$value.observer_role=$role;$value.observer_member=$member;$value.observer_failure=$failure
    Check (Valid $value) "observer registry $role $member $failure"
}}}
$base=Join-Path ([IO.Path]::GetTempPath()) ('owner-refusal-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $base)
$testTransportAllocator=${function:New-CaptureHistoricalTransportPath}
function New-CaptureHistoricalTransportPath { & $testTransportAllocator $base }
try {
 foreach($case in @('valid','malformed','empty','absent','read-timeout','copy-timeout','later-loss')){
    $packet=Join-Path $base $case;[void](New-Item -ItemType Directory -Path $packet)
    $fixtureBytes=[Text.Encoding]::UTF8.GetBytes($(if($case -ceq 'malformed'){'broken'}elseif($case -ceq 'empty'){''}else{$raw}))
    $hash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($fixtureBytes)).ToLowerInvariant()
    $fixtureMetadata="present $($fixtureBytes.Length) $hash 11 99`n";$reads=0
    $record=[ordered]@{capture_verified=$false;facts_verified=$false}
    $read={param($command)$script:reads++;if($case -ceq 'read-timeout' -or ($case -ceq 'later-loss' -and $script:reads -gt 1)){return @{timeout=$true;exit=-1;stdout=''}};@{timeout=$false;exit=0;stdout=$(if($case -ceq 'absent'){"absent`n"}else{$fixtureMetadata})}}
    $copy={param($remote,$local)if($case -ceq 'copy-timeout'){return @{timeout=$true;exit=-1}};[IO.File]::WriteAllBytes($local,$fixtureBytes);@{timeout=$false;exit=0}}
    $identity=[pscustomobject]@{attempt_pid='1234';attempt_start='5678';root_device='11';root_inode='22'}
    $threw=$false;try{Receive-CaptureOwnerRefusalEvidence ('/run/rmb-qt-probe-'+$nonce) $nonce $identity $packet $record $read $copy}catch{$threw=$true}
    Check ($threw -eq ($case -cin @('read-timeout','copy-timeout','later-loss'))) "collector refusal $case"
    Check (-not $record.capture_verified -and -not $record.facts_verified) "no admission $case"
    Check ([bool]$record.capture_owner_refusal_saved_copy_verified -eq ($case -cin @('valid','malformed','empty','later-loss'))) "preservation $case"
    Check ([bool]$record.capture_owner_refusal_decoded -eq ($case -ceq 'valid')) "strict decode $case"
    if($case -cin @('valid','malformed','empty')){
        $preservationRead={param($command)@{exit=0;timeout=$false;stdout=$(if($command.Contains('capture-owner-refusal.json')){"present $($fixtureBytes.Length) $hash`n"}else{"absent`n"})}}
        Check (Test-CapturePreserved ('/run/rmb-qt-probe-'+$nonce) $packet $record $preservationRead) "known bytes cleanup eligible $case"
        $record.capture_owner_refusal_saved_copy_verified=$false
        Check (-not(Test-CapturePreserved ('/run/rmb-qt-probe-'+$nonce) $packet $record $preservationRead)) "unknown bytes retain $case"
    }
 }
}finally{
    $resolved=[IO.Path]::GetFullPath($base);$prefix=[IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if(-not $resolved.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -cnotmatch '\Aowner-refusal-[0-9a-f]{32}\z'){throw 'Fixture cleanup path refused'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
Write-Output "PASS capture owner refusal $count checks"
