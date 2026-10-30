function Test-PageOpenDiagnostics($Diagnostics) {
    if ($null -eq $Diagnostics -or $Diagnostics.terminal_stage -isnot [string] -or
        $Diagnostics.terminal_stage -cne 'open-observed' -or $Diagnostics.access_anchor -isnot [string] -or
        $Diagnostics.access_anchor -cne 'setup-arm-observation' -or
        $Diagnostics.PSObject.Properties.Name -ccontains 'creation_trial') { return $false }
    $trial=$Diagnostics.page_open_trial
    if ($null -eq $trial) { return $false }
    foreach ($field in @('enabled','setup_gate_enabled','target_observed')) {
        if ($trial.$field -isnot [bool] -or -not $trial.$field) { return $false }
    }
    foreach ($field in @('render_authority','native_api_qualified')) {
        if ($trial.$field -isnot [bool] -or $trial.$field) { return $false }
    }
    if ($trial.native_call_attempted -isnot [bool] -or $trial.native_call_returned -isnot [bool] -or
        $trial.native_call_attempted -ne $trial.native_call_returned) { return $false }
    $accepted=$trial.arm_accepted_at_ms; $elapsed=$Diagnostics.elapsed_ms
    if (($accepted -isnot [int] -and $accepted -isnot [long]) -or $accepted -lt 0 -or $accepted -ge 20000 -or
        ($elapsed -isnot [int] -and $elapsed -isnot [long]) -or $elapsed -lt $accepted -or $elapsed -ge ($accepted+5000)) { return $false }
    return $true
}
