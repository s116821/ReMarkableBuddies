# Main freezes the resulting config with the selected SDK source/artifact packet.
function Get-CaptureObservationBuildConfig($Expected,[bool]$FocusAncestry=$false,[bool]$ReceiverSubtreeCapture=$false,[bool]$ReceiverSubtreeCapture512=$false,[int]$ReceiverSubtreeItemCap=0,[int]$ReceiverSubtreeDepthCap=0,[bool]$ReceiverSubtreeCaptureAllowUnfocusedArea=$false,[bool]$ReceiverSubtreeCaptureDetailedIdentityDiagnostics=$false) {
    if($ReceiverSubtreeCaptureDetailedIdentityDiagnostics -and (-not $ReceiverSubtreeCaptureAllowUnfocusedArea -or -not $ReceiverSubtreeCapture -or $ReceiverSubtreeItemCap -ne 4096 -or $ReceiverSubtreeDepthCap -ne 32 -or $ReceiverSubtreeCapture512 -or $FocusAncestry)){throw 'Detailed identity diagnostics require unfocused-area base4096/depth32 without512/focus'}
    if($ReceiverSubtreeCaptureAllowUnfocusedArea -and (-not $ReceiverSubtreeCapture -or $ReceiverSubtreeItemCap -ne 4096 -or $ReceiverSubtreeDepthCap -ne 32 -or $ReceiverSubtreeCapture512 -or $FocusAncestry)){throw 'Unfocused-area capture requires base4096/depth32 without512/focus'}
    if($ReceiverSubtreeItemCap -eq 4096 -and $ReceiverSubtreeDepthCap -notin @(16,32)){throw '4096 profile requires depth16 or32'}
    if($ReceiverSubtreeDepthCap -notin @(0,16,32) -or ($ReceiverSubtreeDepthCap -eq 32 -and $ReceiverSubtreeItemCap -ne 4096) -or ($ReceiverSubtreeDepthCap -ne 0 -and (-not $ReceiverSubtreeCapture -or ($ReceiverSubtreeItemCap -notin @(2048,4096)) -or $ReceiverSubtreeCapture512))){throw 'Explicit depth requires base receiver capture without512; depth16 admits2048/4096 and depth32 only4096'}
    if($ReceiverSubtreeItemCap -notin @(0,1024,2048,4096) -or ($ReceiverSubtreeItemCap -ne 0 -and (-not $ReceiverSubtreeCapture -or $ReceiverSubtreeCapture512))){throw 'Explicit item cap requires1024/2048/4096 receiver profile without512 flag'}
    if($ReceiverSubtreeCapture512 -and -not $ReceiverSubtreeCapture){throw '512 capture profile requires receiver selection'}
    if($FocusAncestry -and $ReceiverSubtreeCapture){throw 'Capture discovery selections are mutually exclusive'}
    $id='\A[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\z'
    if($Expected.nonce -isnot [string] -or $Expected.nonce -cnotmatch '\A[0-9a-f]{32}\z' -or $Expected.document -isnot [string] -or $Expected.document -cnotmatch $id -or $Expected.document -ceq '00000000-0000-0000-0000-000000000000' -or $Expected.order -isnot [array] -or $Expected.order.Count -ne 6){throw 'Fixed capture fixture config refused'}
    if(@($Expected.order|Select-Object -Unique).Count -ne 6){throw 'Duplicate expected page'}
    foreach($page in $Expected.order){if($page -isnot [string] -or $page -cnotmatch $id -or $page -ceq '00000000-0000-0000-0000-000000000000'){throw 'Expected page refused'}}
    $lines=@(
        'inline qml_access::FactsEntryConfig pageFactsEntryConfig() {',
        '    qml_access::FactsEntryConfig config;',
        ('    config.nonce=QStringLiteral("{0}");' -f $Expected.nonce),
        '    config.directory=QStringLiteral("/run/rmb-qt-probe-")+config.nonce;',
        ('    config.facts.documentId=QStringLiteral("{0}");' -f $Expected.document),
        '    config.facts.pageCap=6; config.facts.budgetMs=5000;',
        '    config.setupBudgetMs=120000; config.developmentSetup120=true;',
        '    config.setupSelection=QStringLiteral("main-dev-facts-120s");',
        '    config.developmentCaptureObservation=true;',
        '    config.developmentInputObservation=false;')
    if($FocusAncestry){$lines+='    config.developmentFocusAncestry=true;'}
    if($ReceiverSubtreeCapture){$lines+='    config.developmentReceiverSubtreeCapture=true;'}
    if($ReceiverSubtreeCapture512){$lines+='    config.developmentReceiverSubtreeCapture512=true;'}
    if($ReceiverSubtreeItemCap -ne 0){$lines+=('    config.developmentReceiverSubtreeItemCap={0};' -f $ReceiverSubtreeItemCap)}
    if($ReceiverSubtreeDepthCap -ne 0){$lines+=('    config.developmentReceiverSubtreeDepthCap='+$ReceiverSubtreeDepthCap+';')}
    if($ReceiverSubtreeCaptureAllowUnfocusedArea){$lines+='    config.developmentReceiverSubtreeCaptureAllowUnfocusedArea=true;'}
    if($ReceiverSubtreeCaptureDetailedIdentityDiagnostics){$lines+='    config.developmentReceiverSubtreeCaptureDetailedIdentityDiagnostics=true;'}
    foreach($page in $Expected.order){$lines+=('    config.facts.expectedOrder.append(QStringLiteral("{0}"));' -f $page)}
    $lines+=@('    return config;','}')
    return ($lines -join "`n")+"`n"
}
