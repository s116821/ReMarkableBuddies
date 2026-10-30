# Fixed development facts evidence only; live generation/recovery remain separate.
function Test-FactsCallback($Callback, [string]$ExpectedNonce) {
    Set-StrictMode -Off
    if ($ExpectedNonce -cnotmatch '^[0-9a-f]{32}$' -or $null -eq $Callback) { return $false }
    return $Callback.nonce -is [string] -and $Callback.nonce -ceq $ExpectedNonce -and
        $Callback.stage -is [string] -and $Callback.stage -ceq 'facts-observed-no-change-during-read' -and
        $Callback.application_thread -is [bool] -and $Callback.application_thread -and
        $Callback.engine_thread -is [bool] -and $Callback.engine_thread -and
        @($Callback.PSObject.Properties).Count -eq 4
}
function Test-FactsDiagnostics($Facts, [string]$ExpectedNonce, [string]$ExpectedDocument, [string[]]$ExpectedOrder,
    [long]$ExpectedPid, [string]$ExpectedStart) {
    Set-StrictMode -Off
    $canonical='^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    $nil='00000000-0000-0000-0000-000000000000'
    if ($ExpectedNonce -cnotmatch '^[0-9a-f]{32}$' -or $ExpectedPid -le 1 -or $ExpectedStart -cnotmatch '^[1-9][0-9]*$' -or
        $ExpectedDocument -cnotmatch $canonical -or $ExpectedDocument -ceq $nil -or
        $ExpectedOrder.Count -ne 6 -or @($ExpectedOrder | Select-Object -Unique).Count -ne 6 -or $null -eq $Facts) { return $false }
    foreach ($id in $ExpectedOrder) { if ($id -cnotmatch $canonical -or $id -ceq $nil) { return $false } }
    if ($Facts.kind -isnot [string] -or $Facts.kind -cne 'development-observed-facts' -or
        $Facts.nonce -isnot [string] -or $Facts.nonce -cne $ExpectedNonce -or
        $Facts.attempt_pid -isnot [string] -or $Facts.attempt_pid -cne [string]$ExpectedPid -or
        $Facts.attempt_start -isnot [string] -or $Facts.attempt_start -cne $ExpectedStart -or
        $Facts.document_id -isnot [string] -or $Facts.document_id -cne $ExpectedDocument -or
        $Facts.order -isnot [array] -or $Facts.order.Count -ne 6 -or
        @($Facts.PSObject.Properties).Count -ne 23 -or
        ($Facts.setup_budget_ms -isnot [int] -and $Facts.setup_budget_ms -isnot [long])) { return $false }
    if ($Facts.development_setup_opt_in -isnot [bool] -or -not $Facts.development_setup_opt_in -or
        $Facts.setup_budget_ms -ne 120000 -or $Facts.setup_selection -isnot [string] -or
        $Facts.setup_selection -cne 'main-dev-facts-120s') { return $false }
    foreach ($field in @('required_metadata_validated','required_connections_installed')) {
        if ($Facts.$field -isnot [bool] -or -not $Facts.$field) { return $false }
    }
    for ($index=0;$index -lt 6;$index++) {
        if ($Facts.order[$index] -isnot [string] -or $Facts.order[$index] -cne $ExpectedOrder[$index]) { return $false }
    }
    foreach ($field in @('current_index','begin_ms','end_ms','request_accepted_ms','delivered_ms')) {
        if ($Facts.$field -isnot [int] -and $Facts.$field -isnot [long]) { return $false }
    }
    if ($Facts.current_index -lt 0 -or $Facts.current_index -ge 6 -or
        $Facts.current_page_id -isnot [string] -or $Facts.current_page_id -cne $ExpectedOrder[$Facts.current_index] -or
        $Facts.begin_ms -lt 0 -or $Facts.end_ms -lt $Facts.begin_ms -or $Facts.end_ms -ge 5000 -or
        $Facts.request_accepted_ms -lt 0 -or $Facts.request_accepted_ms -ge 120000 -or
        $Facts.delivered_ms -lt $Facts.request_accepted_ms -or $Facts.delivered_ms -ge ($Facts.request_accepted_ms+5000)) { return $false }
    foreach ($field in @('atomic_snapshot','native_authority','render_authority')) {
        if ($Facts.$field -isnot [bool] -or $Facts.$field) { return $false }
    }
    foreach ($field in @('instance','begin_epoch','end_epoch')) {
        if ($Facts.$field -isnot [string] -or $Facts.$field -cnotmatch '^(0|[1-9][0-9]{0,19})$') { return $false }
        $value=0UL
        if (-not [UInt64]::TryParse($Facts.$field,[ref]$value)) { return $false }
    }
    return $Facts.instance -cne '0' -and $Facts.begin_epoch -ceq $Facts.end_epoch
}
