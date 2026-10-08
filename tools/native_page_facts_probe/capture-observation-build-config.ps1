# Main freezes the resulting config with the selected SDK source/artifact packet.
function Get-CaptureObservationBuildConfig($Expected,[bool]$FocusAncestry=$false,[bool]$ReceiverSubtreeCapture=$false,[bool]$ReceiverSubtreeCapture512=$false) {
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
    foreach($page in $Expected.order){$lines+=('    config.facts.expectedOrder.append(QStringLiteral("{0}"));' -f $page)}
    $lines+=@('    return config;','}')
    return ($lines -join "`n")+"`n"
}
