# Historical first-failure diagnostics only; no admission or native authority.
function ConvertFrom-CaptureOwnerRefusalRaw([string]$Raw) {
    if([Text.Encoding]::UTF8.GetByteCount($Raw) -gt 8192){return $null}
    $document=$null
    try{
        $document=[System.Text.Json.JsonDocument]::Parse($Raw)
        if($document.RootElement.ValueKind -ne [System.Text.Json.JsonValueKind]::Object){return $null}
        $names=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach($property in $document.RootElement.EnumerateObject()){if(-not $names.Add($property.Name)){return $null}}
        return ConvertFrom-Json -InputObject $Raw -ErrorAction Stop
    }catch{return $null}finally{if($null -ne $document){$document.Dispose()}}
}
function Test-CaptureOwnerRefusal($Value,[string]$Nonce,[string]$PidText,[string]$Started,[string]$Dev,[string]$Ino) {
    if($null -eq $Value){return $false}
    $fields=@('kind','version','nonce','attempt_pid','attempt_start','root_device','root_inode','setup_profile','capture_accepted_ms','failure_ms','deadline_check_ms','effective_deadline_ms','branch','predicate','discovery_result','visited_items','receiver_candidates','scene_candidates','matched_pairs','first_pair_receiver','first_pair_scene','first_pair_rejection','active_owner_rejection','observer_role','observer_member','observer_failure','native_authority','render_authority','ui_acknowledged')
    $names=@($Value.PSObject.Properties.Name)
    if($names.Count -ne 29 -or @($names|Where-Object {$_ -cnotin $fields}).Count){return $false}
    foreach($name in @('kind','nonce','setup_profile','branch')){if($Value.$name -isnot [string]){return $false}}
    if($Value.kind -cne 'development-capture-owner-refusal' -or $Value.nonce -cne $Nonce -or $Nonce -cnotmatch '\A[0-9a-f]{32}\z' -or $Value.setup_profile -cne 'main-dev-facts-120s'){return $false}
    foreach($name in @('attempt_pid','attempt_start','root_device','root_inode')){
        if($Value.$name -isnot [string] -or $Value.$name -cnotmatch '\A[1-9][0-9]{0,19}\z'){return $false}
        $parsed=[ulong]0;if(-not [ulong]::TryParse($Value.$name,[ref]$parsed)){return $false}
    }
    if($Value.attempt_pid -cne $PidText -or $Value.attempt_start -cne $Started -or $Value.root_device -cne $Dev -or $Value.root_inode -cne $Ino){return $false}
    foreach($name in @('native_authority','render_authority','ui_acknowledged')){if($Value.$name -isnot [bool] -or $Value.$name){return $false}}
    foreach($name in @('version','capture_accepted_ms','failure_ms','effective_deadline_ms')){if(($Value.$name -isnot [int] -and $Value.$name -isnot [long]) -or $Value.$name -lt 0){return $false}}
    if($Value.version -ne 1 -or $Value.capture_accepted_ms -ge 120000 -or $Value.failure_ms -lt $Value.capture_accepted_ms -or $Value.effective_deadline_ms -ne [Math]::Min(120000,$Value.capture_accepted_ms+5000)){return $false}
    $progress=@('context','live','invalidated','token','deadline')
    $allowed=@('reentrant-check','invalidated-before','context-before','lifetime-before','active-owner','invalidated-after','context-after','lifetime-after','owner-pointers','owner-threads','deadline')
    $discovery=@('open-engine-thread','open-current-window-unavailable','open-context-lost','open-item-lost','open-topology-bound','open-candidate-bound','open-owner-ambiguous','open-owner-unavailable')
    $groups=@('pointers-or-threads','window-focus-active-visible','engine-association','window-association','item-visible-enabled','scene-active-focus','receiver-ancestor','drawing-area-focused','receiver-document','scene-document','document-identity')
    foreach($name in @('predicate','discovery_result','first_pair_rejection','active_owner_rejection','observer_role','observer_member','observer_failure')){if($null -ne $Value.$name -and $Value.$name -isnot [string]){return $false}}
    $counters=@('visited_items','receiver_candidates','scene_candidates','matched_pairs')
    $pair=@('first_pair_receiver','first_pair_scene','first_pair_rejection')
    $observer=@('observer_role','observer_member','observer_failure')
    $bounds=@{visited_items=4097;receiver_candidates=9;scene_candidates=9;matched_pairs=2;first_pair_receiver=7;first_pair_scene=7}
    foreach($name in $bounds.Keys){if($null -ne $Value.$name -and (($Value.$name -isnot [int] -and $Value.$name -isnot [long]) -or $Value.$name -lt 0 -or $Value.$name -gt $bounds[$name])){return $false}}
    if($null -ne $Value.active_owner_rejection -and $Value.active_owner_rejection -cnotin $groups){return $false}
    $pairCount=@($pair|Where-Object {$null -ne $Value.$_}).Count
    $observerCount=@($observer|Where-Object {$null -ne $Value.$_}).Count
    if($pairCount -notin @(0,3) -or $observerCount -notin @(0,3)){return $false}
    switch -CaseSensitive ($Value.branch){
      'initial-progress' {
        if($Value.predicate -cnotin $progress -or $null -ne $Value.discovery_result){return $false}
        foreach($name in ($counters+$pair+$observer+@('active_owner_rejection'))){if($null -ne $Value.$name){return $false}}
      }
      'owner-discovery' {
        if($Value.discovery_result -cnotin $discovery -or $observerCount){return $false}
        if($null -ne $Value.predicate -and ($Value.predicate -cnotin $progress -or $Value.discovery_result -cne 'open-context-lost')){return $false}
        if($null -ne $Value.active_owner_rejection -and ($Value.discovery_result -cne 'open-context-lost' -or $null -ne $Value.predicate)){return $false}
        $visitedCount=@(@('visited_items','receiver_candidates','scene_candidates')|Where-Object {$null -ne $Value.$_}).Count
        if($visitedCount -notin @(0,3) -or ($visitedCount -eq 0 -and $null -ne $Value.matched_pairs)){return $false}
        if($Value.discovery_result -cin @('open-engine-thread','open-current-window-unavailable') -and $visitedCount){return $false}
        if($Value.discovery_result -cin @('open-owner-unavailable','open-owner-ambiguous')){
            if($visitedCount -ne 3 -or $null -eq $Value.matched_pairs -or $Value.matched_pairs -ne $(if($Value.discovery_result -ceq 'open-owner-unavailable'){0}else{2})){return $false}
        }
        if($pairCount){
            if($Value.discovery_result -cne 'open-owner-unavailable' -or $Value.matched_pairs -ne 0 -or $Value.first_pair_rejection -cnotin $groups -or $Value.first_pair_receiver -ge $Value.receiver_candidates -or $Value.first_pair_scene -ge $Value.scene_candidates){return $false}
        }
      }
      'observer-install' {
        if($null -ne $Value.predicate -or $Value.discovery_result -cne 'open-owner-observed' -or $observerCount -ne 3){return $false}
        foreach($name in ($counters+$pair+@('active_owner_rejection'))){if($null -ne $Value.$name){return $false}}
        $members=@{
            document=@('pageCountChanged(int,int)','pageMapChanged()','pageAdded(int)','pagesAdded(QList<int>)','pageMoved(int,int)','pagesMoved()','pagesRemoved()','redirectionPageMapChanged()','pageUpdated(int)','documentMetadataChanged()','orientationChanged()')
            scene=@('pageIdChanged()','documentWrapperChanged()','workerChanged()','viewportChanged()')
            receiver=@('document','currentPage','currentPageId','drawingAreaFocused')
        }
        if($Value.observer_role -cnotin @('document','scene','receiver') -or $Value.observer_member -cnotin $members[$Value.observer_role] -or $Value.observer_failure -cnotin @('object-missing','property-missing','notify-missing','signal-missing','slot-missing','return-type','connect-failed')){return $false}
      }
      'owner-revalidation' {
        if($Value.predicate -cnotin $allowed -or $Value.discovery_result -cne 'open-owner-observed'){return $false}
        foreach($name in ($counters+$pair+$observer)){if($null -ne $Value.$name){return $false}}
        if(($Value.predicate -ceq 'active-owner') -ne ($null -ne $Value.active_owner_rejection)){return $false}
      }
      default{return $false}
    }
    if($Value.predicate -ceq 'deadline'){
        if(($Value.deadline_check_ms -isnot [int] -and $Value.deadline_check_ms -isnot [long]) -or $Value.deadline_check_ms -lt $Value.effective_deadline_ms -or $Value.deadline_check_ms -gt $Value.failure_ms){return $false}
    }elseif($null -ne $Value.deadline_check_ms){return $false}
    return $true
}
