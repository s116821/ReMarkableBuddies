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
function Test-CaptureOwnerRefusal($Value,[string]$Nonce,[string]$PidText,[string]$Started,[string]$Dev,[string]$Ino,[bool]$FocusAncestry=$false,[bool]$ReceiverSubtreeCapture=$false,[bool]$ReceiverSubtreeCapture512=$false,[int]$ReceiverSubtreeItemCap=0,[int]$ReceiverSubtreeDepthCap=0,[bool]$ReceiverSubtreeCaptureAllowUnfocusedArea=$false,[bool]$ReceiverSubtreeCaptureDetailedIdentityDiagnostics=$false,[bool]$ReceiverSubtreeCaptureDocumentIdTypeDiagnostics=$false,[bool]$ReceiverSubtreeCaptureEntryIdConversion=$false) {
    if($null -eq $Value){return $false}
    $fields=@('kind','version','nonce','attempt_pid','attempt_start','root_device','root_inode','setup_profile','capture_accepted_ms','failure_ms','deadline_check_ms','effective_deadline_ms','branch','predicate','discovery_result','visited_items','receiver_candidates','scene_candidates','matched_pairs','first_pair_receiver','first_pair_scene','first_pair_rejection','active_owner_rejection','observer_role','observer_member','observer_failure','native_authority','render_authority','ui_acknowledged')
    if($ReceiverSubtreeCaptureEntryIdConversion -and (-not $ReceiverSubtreeCaptureDocumentIdTypeDiagnostics -or -not $ReceiverSubtreeCaptureDetailedIdentityDiagnostics -or -not $ReceiverSubtreeCaptureAllowUnfocusedArea -or -not $ReceiverSubtreeCapture -or $ReceiverSubtreeItemCap -ne 4096 -or $ReceiverSubtreeDepthCap -ne 32 -or $ReceiverSubtreeCapture512 -or $FocusAncestry)){return $false}
    if($ReceiverSubtreeCaptureDocumentIdTypeDiagnostics -and (-not $ReceiverSubtreeCaptureDetailedIdentityDiagnostics -or -not $ReceiverSubtreeCaptureAllowUnfocusedArea -or -not $ReceiverSubtreeCapture -or $ReceiverSubtreeItemCap -ne 4096 -or $ReceiverSubtreeDepthCap -ne 32 -or $ReceiverSubtreeCapture512 -or $FocusAncestry)){return $false}
    if($ReceiverSubtreeCaptureDetailedIdentityDiagnostics -and (-not $ReceiverSubtreeCaptureAllowUnfocusedArea -or -not $ReceiverSubtreeCapture -or $ReceiverSubtreeItemCap -ne 4096 -or $ReceiverSubtreeDepthCap -ne 32 -or $ReceiverSubtreeCapture512 -or $FocusAncestry)){return $false}
    if($ReceiverSubtreeCaptureAllowUnfocusedArea -and (-not $ReceiverSubtreeCapture -or $ReceiverSubtreeItemCap -ne 4096 -or $ReceiverSubtreeDepthCap -ne 32 -or $ReceiverSubtreeCapture512 -or $FocusAncestry)){return $false}
    if($ReceiverSubtreeItemCap -eq 4096 -and $ReceiverSubtreeDepthCap -notin @(16,32)){return $false}
    if($ReceiverSubtreeDepthCap -notin @(0,16,32) -or ($ReceiverSubtreeDepthCap -eq 32 -and $ReceiverSubtreeItemCap -ne 4096) -or ($ReceiverSubtreeDepthCap -ne 0 -and (-not $ReceiverSubtreeCapture -or ($ReceiverSubtreeItemCap -notin @(2048,4096)) -or $ReceiverSubtreeCapture512))){return $false}
    if($ReceiverSubtreeItemCap -notin @(0,1024,2048,4096) -or ($ReceiverSubtreeItemCap -ne 0 -and (-not $ReceiverSubtreeCapture -or $ReceiverSubtreeCapture512))){return $false}
    if($ReceiverSubtreeCapture512 -and -not $ReceiverSubtreeCapture){return $false}
    if($FocusAncestry -and $ReceiverSubtreeCapture){return $false}
    if(($Value.version -isnot [int] -and $Value.version -isnot [long]) -or $Value.version -notin @(1,2,3,4,5) -or ($Value.version -in @(3,4) -and -not $FocusAncestry) -or ($Value.version -eq 5 -and -not $ReceiverSubtreeCapture) -or ($ReceiverSubtreeCapture -and $Value.version -ne 5)){return $false}
    $topologyFields=@('topology_limit','topology_depth','topology_queue_size','topology_child_count')
    if($Value.version -in @(2,3,4,5)){$fields+=$topologyFields}
    if($Value.version -in @(3,4)){$fields+=@('discovery_scope','chain_items','chain_complete','chain_failure')}
    $sceneFields=@('scene_rejected_engine','scene_rejected_class','scene_rejected_page_id','scene_rejected_page_id_changed','scene_rejected_document_wrapper_changed','scene_passed')
    if($Value.version -eq 4){$fields+=$sceneFields}
    if($Value.version -eq 5){$fields+='discovery_scope'}
    $names=@($Value.PSObject.Properties.Name)
    if($names.Count -ne $fields.Count -or @($names|Where-Object {$_ -cnotin $fields}).Count){return $false}
    foreach($name in @('kind','nonce','setup_profile','branch')){if($Value.$name -isnot [string]){return $false}}
    if($Value.kind -cne 'development-capture-owner-refusal' -or $Value.nonce -cne $Nonce -or $Nonce -cnotmatch '\A[0-9a-f]{32}\z' -or $Value.setup_profile -cne 'main-dev-facts-120s'){return $false}
    foreach($name in @('attempt_pid','attempt_start','root_device','root_inode')){
        if($Value.$name -isnot [string] -or $Value.$name -cnotmatch '\A[1-9][0-9]{0,19}\z'){return $false}
        $parsed=[ulong]0;if(-not [ulong]::TryParse($Value.$name,[ref]$parsed)){return $false}
    }
    if($Value.attempt_pid -cne $PidText -or $Value.attempt_start -cne $Started -or $Value.root_device -cne $Dev -or $Value.root_inode -cne $Ino){return $false}
    foreach($name in @('native_authority','render_authority','ui_acknowledged')){if($Value.$name -isnot [bool] -or $Value.$name){return $false}}
    foreach($name in @('version','capture_accepted_ms','failure_ms','effective_deadline_ms')){if(($Value.$name -isnot [int] -and $Value.$name -isnot [long]) -or $Value.$name -lt 0){return $false}}
    if($Value.capture_accepted_ms -ge 120000 -or $Value.failure_ms -lt $Value.capture_accepted_ms -or $Value.effective_deadline_ms -ne [Math]::Min(120000,$Value.capture_accepted_ms+5000)){return $false}
    $progress=@('context','live','invalidated','token','deadline')
    $allowed=@('reentrant-check','invalidated-before','context-before','lifetime-before','active-owner','invalidated-after','context-after','lifetime-after','owner-pointers','owner-threads','deadline')
    $discovery=@('open-engine-thread','open-current-window-unavailable','open-context-lost','open-item-lost','open-topology-bound','open-candidate-bound','open-owner-ambiguous','open-owner-unavailable')
    $groups=@('pointers-or-threads','window-focus-active-visible','engine-association','window-association','item-visible-enabled','scene-active-focus','receiver-ancestor','drawing-area-focused','receiver-document','scene-document','document-identity')
    foreach($name in @('predicate','discovery_result','first_pair_rejection','active_owner_rejection','observer_role','observer_member','observer_failure')){if($null -ne $Value.$name -and $Value.$name -isnot [string]){return $false}}
    $counters=@('visited_items','receiver_candidates','scene_candidates','matched_pairs')
    $pair=@('first_pair_receiver','first_pair_scene','first_pair_rejection')
    $observer=@('observer_role','observer_member','observer_failure')
    if($Value.version -eq 5){return Test-ReceiverSubtreeCaptureRefusalFields $Value $progress $allowed $groups $topologyFields $counters $pair $observer $ReceiverSubtreeCapture512 $ReceiverSubtreeItemCap $ReceiverSubtreeDepthCap $ReceiverSubtreeCaptureAllowUnfocusedArea $ReceiverSubtreeCaptureDetailedIdentityDiagnostics $ReceiverSubtreeCaptureDocumentIdTypeDiagnostics $ReceiverSubtreeCaptureEntryIdConversion}
    if($Value.version -in @(3,4)){
        foreach($name in ($counters+@('first_pair_receiver','first_pair_scene'))){
            if($null -ne $Value.$name -and (($Value.$name -isnot [int] -and $Value.$name -isnot [long]) -or $Value.$name -lt 0)){return $false}
        }
        if($Value.discovery_scope -isnot [string] -or $Value.discovery_scope -cne 'window-focus-ancestry-v1' -or $Value.chain_complete -isnot [bool]){return $false}
        foreach($name in $topologyFields){if($null -ne $Value.$name){return $false}}
        if($null -ne $Value.chain_items -and (($Value.chain_items -isnot [int] -and $Value.chain_items -isnot [long]) -or $Value.chain_items -lt 0 -or $Value.chain_items -gt 25)){return $false}
        if($null -ne $Value.chain_failure -and ($Value.chain_failure -isnot [string] -or $Value.chain_failure -cnotin @('anchor-unavailable','item-context','root-unreached','depth-bound','cycle','observer-unavailable'))){return $false}
        if($Value.branch -ceq 'initial-progress'){
            if($null -ne $Value.chain_items -or $Value.chain_complete -or $null -ne $Value.chain_failure){return $false}
        }elseif($Value.branch -ceq 'owner-discovery'){
            if($null -eq $Value.chain_items){return $false}
            if(-not $Value.chain_complete){
                foreach($name in ($counters+$pair+$observer+@('active_owner_rejection'))){if($null -ne $Value.$name){return $false}}
                if($Value.discovery_result -ceq 'open-focus-chain-refused'){
                    if($null -eq $Value.chain_failure -or $null -ne $Value.predicate){return $false}
                    switch -CaseSensitive ($Value.chain_failure){
                        'anchor-unavailable' {if($Value.chain_items -ne 0){return $false}}
                        'root-unreached' {if($Value.chain_items -lt 1){return $false}}
                        'cycle' {if($Value.chain_items -lt 1){return $false}}
                        'depth-bound' {if($Value.chain_items -ne 25){return $false}}
                    }
                    $discovery+=@('open-focus-chain-refused')
                }elseif($Value.discovery_result -cne 'open-context-lost' -or $Value.predicate -cnotin $progress -or $null -ne $Value.chain_failure){return $false}
            }else{
                if($Value.chain_items -lt 1 -or $null -ne $Value.chain_failure -or $Value.discovery_result -cnotin @('open-context-lost','open-item-lost','open-candidate-bound','open-owner-unavailable','open-owner-ambiguous')){return $false}
                foreach($name in @('visited_items','receiver_candidates','scene_candidates')){if($null -eq $Value.$name){return $false}}
                if($Value.visited_items -gt $Value.chain_items){return $false}
                if($null -ne $Value.matched_pairs -and ($Value.visited_items -ne $Value.chain_items -or $Value.receiver_candidates -gt 8 -or $Value.scene_candidates -gt 8)){return $false}
            }
        }elseif($Value.branch -cin @('observer-install','owner-revalidation')){
            if(-not $Value.chain_complete -or $null -eq $Value.chain_items -or $Value.chain_items -lt 1 -or $null -ne $Value.chain_failure){return $false}
        }else{return $false}
    }
    if($Value.version -eq 4){
        $classified=$Value.branch -ceq 'owner-discovery' -and $null -ne $Value.visited_items
        $sceneTotal=0
        foreach($name in $sceneFields){
            if($classified){
                if(($Value.$name -isnot [int] -and $Value.$name -isnot [long]) -or $Value.$name -lt 0 -or $Value.$name -gt 25){return $false}
                $sceneTotal+=$Value.$name
            }elseif($null -ne $Value.$name){return $false}
        }
        if($classified -and ($sceneTotal -ne $Value.visited_items -or $Value.scene_passed -ne $Value.scene_candidates)){return $false}
    }
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
        if($Value.discovery_result -cin @('open-item-lost','open-topology-bound','open-candidate-bound') -and ($visitedCount -ne 3 -or $null -ne $Value.matched_pairs)){return $false}
        if($Value.discovery_result -ceq 'open-candidate-bound' -and $Value.receiver_candidates -ne 9 -and $Value.scene_candidates -ne 9){return $false}
        # Overflow is observed only at its immediate original refusal. No later
        # traversal/pair/final-owner outcome can inherit those counts.
        if($visitedCount -eq 3){
            if($Value.receiver_candidates -gt $Value.visited_items -or $Value.scene_candidates -gt $Value.visited_items){return $false}
            if($Value.discovery_result -cne 'open-topology-bound' -and $Value.visited_items -gt 4096){return $false}
            if($Value.discovery_result -cne 'open-candidate-bound' -and ($Value.receiver_candidates -gt 8 -or $Value.scene_candidates -gt 8)){return $false}
        }
        if($Value.discovery_result -ceq 'open-topology-bound' -and $Value.visited_items -lt 1){return $false}
        if($Value.discovery_result -ceq 'open-context-lost'){
            if($null -ne $Value.matched_pairs -and ($Value.matched_pairs -gt 1 -or $Value.visited_items -lt 1 -or $Value.receiver_candidates -lt 1 -or $Value.scene_candidates -lt 1)){return $false}
            if($null -ne $Value.active_owner_rejection -and $Value.matched_pairs -ne 1){return $false}
        }
        if($Value.discovery_result -cin @('open-owner-unavailable','open-owner-ambiguous')){
            if($visitedCount -ne 3 -or $null -eq $Value.matched_pairs -or $Value.matched_pairs -ne $(if($Value.discovery_result -ceq 'open-owner-unavailable'){0}else{2})){return $false}
            if($Value.visited_items -lt 1){return $false}
            if($Value.discovery_result -ceq 'open-owner-ambiguous' -and ($Value.receiver_candidates*$Value.scene_candidates) -lt 2){return $false}
        }
        if($pairCount){
            if($Value.discovery_result -cne 'open-owner-unavailable' -or $Value.matched_pairs -ne 0 -or $Value.first_pair_rejection -cnotin $groups -or $Value.first_pair_receiver -ge $Value.receiver_candidates -or $Value.first_pair_scene -ge $Value.scene_candidates){return $false}
        }
        if($Value.discovery_result -ceq 'open-owner-unavailable' -and $Value.receiver_candidates -gt 0 -and $Value.scene_candidates -gt 0 -and $pairCount -ne 3){return $false}
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
    if($Value.version -eq 2){
        foreach($name in @('topology_depth','topology_queue_size','topology_child_count')){
            if($null -ne $Value.$name -and (($Value.$name -isnot [int] -and $Value.$name -isnot [long]) -or $Value.$name -lt 0)){return $false}
        }
        if($Value.branch -cne 'owner-discovery' -or $Value.discovery_result -cne 'open-topology-bound'){
            foreach($name in $topologyFields){if($null -ne $Value.$name){return $false}}
        }else{
            if($Value.topology_limit -isnot [string]){return $false}
            switch -CaseSensitive ($Value.topology_limit){
                'visited' {
                    if($Value.visited_items -ne 4097 -or $null -ne $Value.topology_depth -or $null -ne $Value.topology_queue_size -or $null -ne $Value.topology_child_count){return $false}
                }
                'depth' {
                    if($Value.visited_items -gt 4096 -or $Value.topology_depth -ne 25 -or $null -ne $Value.topology_queue_size -or $null -ne $Value.topology_child_count){return $false}
                }
                'queue-cap' {
                    if($Value.visited_items -gt 4096 -or $null -eq $Value.topology_depth -or $Value.topology_depth -gt 24 -or $null -eq $Value.topology_queue_size -or $Value.topology_queue_size -lt 1 -or $Value.topology_queue_size -gt 4096 -or $Value.visited_items -gt $Value.topology_queue_size -or $null -eq $Value.topology_child_count -or $Value.topology_child_count -le (4096-$Value.topology_queue_size)){return $false}
                }
                default{return $false}
            }
        }
    }
    if($Value.predicate -ceq 'deadline'){
        if(($Value.deadline_check_ms -isnot [int] -and $Value.deadline_check_ms -isnot [long]) -or $Value.deadline_check_ms -lt $Value.effective_deadline_ms -or $Value.deadline_check_ms -gt $Value.failure_ms){return $false}
    }elseif($null -ne $Value.deadline_check_ms){return $false}
    return $true
}
function Test-ReceiverSubtreeCaptureRefusalFields($v,$progress,$allowed,$groups,$topologyFields,$counters,$pair,$observer,[bool]$ReceiverSubtreeCapture512=$false,[int]$ReceiverSubtreeItemCap=0,[int]$ReceiverSubtreeDepthCap=0,[bool]$ReceiverSubtreeCaptureAllowUnfocusedArea=$false,[bool]$ReceiverSubtreeCaptureDetailedIdentityDiagnostics=$false,[bool]$ReceiverSubtreeCaptureDocumentIdTypeDiagnostics=$false,[bool]$ReceiverSubtreeCaptureEntryIdConversion=$false) {
    if($v.discovery_scope -isnot [string] -or $v.discovery_scope -cne $(if($ReceiverSubtreeCaptureEntryIdConversion){'receiver-subtree-capture-unqualified-v11'}elseif($ReceiverSubtreeCaptureDocumentIdTypeDiagnostics){'receiver-subtree-capture-unqualified-v10'}elseif($ReceiverSubtreeCaptureDetailedIdentityDiagnostics){'receiver-subtree-capture-unqualified-v9'}elseif($ReceiverSubtreeCaptureAllowUnfocusedArea){'receiver-subtree-capture-unqualified-v8'}elseif($ReceiverSubtreeDepthCap -eq 32){'receiver-subtree-capture-unqualified-v7'}elseif($ReceiverSubtreeItemCap -eq 4096){'receiver-subtree-capture-unqualified-v6'}elseif($ReceiverSubtreeDepthCap -eq 16){'receiver-subtree-capture-unqualified-v5'}elseif($ReceiverSubtreeItemCap -eq 2048){'receiver-subtree-capture-unqualified-v4'}elseif($ReceiverSubtreeItemCap -eq 1024){'receiver-subtree-capture-unqualified-v3'}elseif($ReceiverSubtreeCapture512){'receiver-subtree-capture-unqualified-v2'}else{'receiver-subtree-capture-unqualified-v1'})){return $false}
    $groups=@($groups|Where-Object {$_ -cne 'scene-active-focus'})+@('capture-context','capture-identity')
    if($ReceiverSubtreeCaptureDetailedIdentityDiagnostics){$groups+=@('identity-document-type','identity-document-mismatch','identity-index-type','identity-index-range','identity-scene-page-type','identity-receiver-page-type','identity-page-alias-mismatch','identity-page-order-mismatch','identity-context')}
    if($ReceiverSubtreeCaptureDocumentIdTypeDiagnostics){$groups+=@('identity-document-type-invalid','identity-document-type-quuid','identity-document-type-other')}
    if($ReceiverSubtreeCaptureEntryIdConversion){$groups+=@('identity-entry-type-unavailable','identity-entry-type-mismatch','identity-entry-converter-unavailable','identity-entry-conversion-failed')}
    $itemCap=if($ReceiverSubtreeItemCap -ne 0){$ReceiverSubtreeItemCap}elseif($ReceiverSubtreeCapture512){512}else{256}
    $depthCap=if($ReceiverSubtreeDepthCap -ne 0){$ReceiverSubtreeDepthCap}else{8}
    $bounds=@{visited_items=$itemCap;receiver_candidates=1;scene_candidates=9;matched_pairs=2;first_pair_receiver=0;first_pair_scene=7;topology_depth=$depthCap;topology_queue_size=$itemCap;topology_child_count=2147483647}
    foreach($name in $bounds.Keys){if($null -ne $v.$name -and (($v.$name -isnot [int] -and $v.$name -isnot [long]) -or $v.$name -lt 0 -or $v.$name -gt $bounds[$name])){return $false}}
    if($null -ne $v.active_owner_rejection -and $v.active_owner_rejection -cnotin $groups){return $false}
    $pc=@($pair|Where-Object {$null -ne $v.$_}).Count;$oc=@($observer|Where-Object {$null -ne $v.$_}).Count
    $vc=@(@('visited_items','receiver_candidates','scene_candidates')|Where-Object {$null -ne $v.$_}).Count
    if($pc -notin @(0,3) -or $oc -notin @(0,3) -or $vc -notin @(0,3)){return $false}
    if($oc){
        $members=@{document=@('pageCountChanged(int,int)','pageMapChanged()','pageAdded(int)','pagesAdded(QList<int>)','pageMoved(int,int)','pagesMoved()','pagesRemoved()','redirectionPageMapChanged()','pageUpdated(int)','documentMetadataChanged()','orientationChanged()');scene=@('pageIdChanged()','documentWrapperChanged()','workerChanged()','viewportChanged()');receiver=@('document','currentPage','currentPageId','drawingAreaFocused')}
        if($v.observer_role -cnotin @('document','scene','receiver') -or $v.observer_member -cnotin $members[$v.observer_role] -or $v.observer_failure -cnotin @('object-missing','property-missing','notify-missing','signal-missing','slot-missing','return-type','connect-failed')){return $false}
    }
    if($v.branch -cne 'owner-discovery' -or $v.discovery_result -cne 'open-capture-subtree-bound'){
        foreach($name in $topologyFields){if($null -ne $v.$name){return $false}}
    }else{
        if($v.topology_limit -isnot [string]){return $false}
        if($vc -ne 3 -or $v.visited_items -lt 1 -or $v.topology_queue_size -lt $v.visited_items -or $v.topology_child_count -lt 1 -or $null -eq $v.topology_depth -or $null -eq $v.topology_queue_size -or $null -eq $v.topology_child_count){return $false}
        switch -CaseSensitive ($v.topology_limit){
            'subtree-depth'{if($v.topology_depth -ne $depthCap){return $false}}
            'subtree-items'{if($v.topology_depth -ge $depthCap -or $v.topology_child_count -le $itemCap-$v.topology_queue_size){return $false}}
            default{return $false}
        }
    }
    switch -CaseSensitive ($v.branch){
        'initial-progress'{
            if($v.predicate -cnotin $progress -or $null -ne $v.discovery_result){return $false}
            foreach($name in ($counters+$pair+$observer+@('active_owner_rejection'))){if($null -ne $v.$name){return $false}}
        }
        'owner-discovery'{
            if($v.discovery_result -cnotin @('open-context-lost','open-item-lost','open-capture-scope-refused','open-capture-receiver-unavailable','open-capture-receiver-ambiguous','open-capture-subtree-bound','open-candidate-bound','open-owner-unavailable','open-owner-ambiguous')){return $false}
            if($null -ne $v.predicate -and ($v.discovery_result -cne 'open-context-lost' -or $v.predicate -cnotin $progress)){return $false}
            if($oc -and $v.discovery_result -cne 'open-capture-scope-refused'){return $false}
            if($v.discovery_result -cin @('open-capture-receiver-unavailable','open-capture-receiver-ambiguous') -and ($vc -or $pc -or $oc -or $null -ne $v.matched_pairs)){return $false}
            if($vc){
                if($v.receiver_candidates -ne 1 -or $v.scene_candidates -gt $v.visited_items){return $false}
                if($v.discovery_result -cne 'open-candidate-bound' -and $v.scene_candidates -gt 8){return $false}
            }elseif($null -ne $v.matched_pairs){return $false}
            if($v.discovery_result -cin @('open-capture-subtree-bound','open-candidate-bound') -and ($vc -ne 3 -or $null -ne $v.matched_pairs)){return $false}
            if($v.discovery_result -ceq 'open-candidate-bound' -and $v.scene_candidates -ne 9){return $false}
            if($v.discovery_result -cin @('open-owner-unavailable','open-owner-ambiguous')){
                if($vc -ne 3 -or $v.visited_items -lt 1 -or $null -eq $v.matched_pairs -or $v.matched_pairs -ne $(if($v.discovery_result -ceq 'open-owner-unavailable'){0}else{2})){return $false}
                if($v.discovery_result -ceq 'open-owner-ambiguous' -and $v.scene_candidates -lt 2){return $false}
            }
            if($v.discovery_result -ceq 'open-context-lost' -and $null -ne $v.matched_pairs -and ($v.matched_pairs -gt 1 -or $vc -ne 3 -or $v.visited_items -lt 1 -or ($v.matched_pairs -eq 1 -and $v.scene_candidates -lt 1))){return $false}
            if($pc -and ($v.discovery_result -cne 'open-owner-unavailable' -or $v.first_pair_rejection -cnotin $groups -or $v.first_pair_scene -ge $v.scene_candidates)){return $false}
            if($v.discovery_result -ceq 'open-owner-unavailable' -and $v.scene_candidates -gt 0 -and $pc -ne 3){return $false}
            if($null -ne $v.active_owner_rejection -and ($v.discovery_result -cne 'open-context-lost' -or $v.matched_pairs -ne 1 -or $null -ne $v.predicate)){return $false}
        }
        'observer-install'{
            if($v.discovery_result -cne 'open-capture-scene-correlated' -or $null -ne $v.predicate -or $oc -ne 3 -or $vc -or $pc -or $null -ne $v.matched_pairs -or $null -ne $v.active_owner_rejection){return $false}
        }
        'owner-revalidation'{
            if($v.discovery_result -cne 'open-capture-scene-correlated' -or $v.predicate -cnotin $allowed -or $oc -or $vc -or $pc -or $null -ne $v.matched_pairs){return $false}
            if(($v.predicate -ceq 'active-owner') -ne ($null -ne $v.active_owner_rejection)){return $false}
        }
        default{return $false}
    }
    if($v.predicate -ceq 'deadline'){
        if(($v.deadline_check_ms -isnot [int] -and $v.deadline_check_ms -isnot [long]) -or $v.deadline_check_ms -lt $v.effective_deadline_ms -or $v.deadline_check_ms -gt $v.failure_ms){return $false}
    }elseif($null -ne $v.deadline_check_ms){return $false}
    return $true
}
