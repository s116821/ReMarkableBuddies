# Fixed diagnostic decoding only. A decoded refusal never grants facts/live authority.
function Get-FactsRefusalStages {
    @('facts-reentry-refused','facts-config-or-context-refused','open-engine-thread',
      'open-current-window-unavailable','open-context-lost','open-item-lost','open-topology-bound',
      'open-candidate-bound','open-owner-ambiguous','open-owner-unavailable','facts-metadata-or-context-refused',
      'facts-helper-refused','facts-changed-or-context-refused','facts-values-refused','facts-final-boundary-refused',
      'facts-delivery-boundary-refused','facts-observed-no-change-during-read','unlisted-reader-stage')
}
function Test-FactsRefusalCallback($Callback,[string]$ExpectedNonce) {
    Set-StrictMode -Off
    return $null -ne $Callback -and @($Callback.PSObject.Properties).Count -eq 4 -and
        $ExpectedNonce -cmatch '^[0-9a-f]{32}$' -and $Callback.nonce -is [string] -and $Callback.nonce -ceq $ExpectedNonce -and
        $Callback.stage -is [string] -and $Callback.stage -cin @('facts-entry-read-refused','facts-entry-delivery-refused') -and
        $Callback.application_thread -is [bool] -and $Callback.engine_thread -is [bool]
}
function ConvertFrom-FactsRefusal([string]$Text,[string]$ExpectedNonce,[long]$ExpectedPid,[string]$ExpectedStart) {
    Set-StrictMode -Off
    if([Text.Encoding]::UTF8.GetByteCount($Text) -gt 2048 -or [string]::IsNullOrEmpty($Text) -or
       $ExpectedNonce -cnotmatch '^[0-9a-f]{32}$' -or $ExpectedPid -le 1 -or $ExpectedStart -cnotmatch '^[1-9][0-9]*$'){return $null}
    # Check raw topology first; ConvertFrom-Json alone can discard duplicate keys.
    $json=$null
    try{
        $json=[System.Text.Json.JsonDocument]::Parse($Text,[System.Text.Json.JsonDocumentOptions]::new())
        if($json.RootElement.ValueKind -ne [System.Text.Json.JsonValueKind]::Object){return $null}
        $rawNames=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach($property in $json.RootElement.EnumerateObject()){
            if(-not $rawNames.Add($property.Name)){return $null}
        }
        if($rawNames.Count -ne 27){return $null}
        $value=$Text|ConvertFrom-Json -ErrorAction Stop
    }catch{return $null}finally{if($null -ne $json){$json.Dispose()}}
    $names=@('kind','version','nonce','attempt_pid','attempt_start','reader_stage','reader_result_had_facts',
       'entry_stage','refusal_path','sample_phase','sample_ms','request_accepted_ms','accepted_read_elapsed_ms',
       'setup_clock_origin','read_clock_origin','setup_budget_ms','read_budget_ms','setup_selection',
       'development_setup_opt_in','entry_context_current','root_identity_current','closure_absent',
       'attempt_identity_current','within_accepted_deadline','atomic_snapshot','native_authority','render_authority')
    if($null -eq $value -or @($value.PSObject.Properties).Count -ne 27){return $null}
    foreach($field in $value.PSObject.Properties.Name){if($field -cnotin $names){return $null}}
    foreach($field in @('kind','nonce','attempt_pid','attempt_start','reader_stage','entry_stage','refusal_path','sample_phase',
       'setup_clock_origin','read_clock_origin','setup_selection')){if($value.$field -isnot [string]){return $null}}
    foreach($field in @('version','sample_ms','request_accepted_ms','accepted_read_elapsed_ms','setup_budget_ms','read_budget_ms')){
        if($value.$field -isnot [int] -and $value.$field -isnot [long]){return $null}
    }
    foreach($field in @('reader_result_had_facts','development_setup_opt_in','entry_context_current','root_identity_current',
       'closure_absent','attempt_identity_current','within_accepted_deadline','atomic_snapshot','native_authority','render_authority')){
        if($value.$field -isnot [bool]){return $null}
    }
    if($value.kind -cne 'development-facts-refusal' -or $value.version -ne 1 -or $value.nonce -cne $ExpectedNonce -or
       $value.attempt_pid -cne [string]$ExpectedPid -or $value.attempt_start -cne $ExpectedStart -or
       $value.reader_stage -cnotin (Get-FactsRefusalStages) -or $value.sample_phase -cne 'before-refusal-callback' -or
       $value.setup_clock_origin -cne 'entry-startup-monotonic' -or $value.read_clock_origin -cne 'accepted-request-monotonic' -or
       $value.setup_budget_ms -ne 120000 -or $value.read_budget_ms -ne 5000 -or
       $value.setup_selection -cne 'main-dev-facts-120s' -or -not $value.development_setup_opt_in -or
       $value.atomic_snapshot -or $value.native_authority -or $value.render_authority){return $null}
    if($value.entry_stage -ceq 'facts-entry-read-refused'){
        $path=if($value.reader_result_had_facts){'entry-read-boundary'}else{'reader-result'}
        if($value.refusal_path -cne $path){return $null}
    }elseif($value.entry_stage -ceq 'facts-entry-delivery-refused'){
        if($value.refusal_path -cne 'entry-completion-boundary' -or -not $value.reader_result_had_facts){return $null}
    }else{return $null}
    if($value.request_accepted_ms -eq -1){
        if($value.accepted_read_elapsed_ms -ne -1 -or $value.sample_ms -lt -1 -or $value.within_accepted_deadline){return $null}
    }elseif($value.request_accepted_ms -lt 0 -or $value.request_accepted_ms -ge 120000 -or
       $value.sample_ms -lt $value.request_accepted_ms -or
       $value.accepted_read_elapsed_ms -ne ($value.sample_ms-$value.request_accepted_ms) -or
       $value.within_accepted_deadline -ne ($value.sample_ms -lt ($value.request_accepted_ms+5000))){return $null}
    return $value # Diagnostic shape/correlation only, never a success predicate.
}
function Get-FactsRefusalReadCommand {
    # Caller uses the original live ObservationSSH clock or final recovery SSH.
    @'
set -eu
if test -f '@ROOT@/refusal.json'; then
    test ! -L '@ROOT@/refusal.json'
    test "$(stat -c '%a %u' '@ROOT@/refusal.json')" = '600 0'
    test "$(wc -c < '@ROOT@/refusal.json')" -gt 0
    test "$(wc -c < '@ROOT@/refusal.json')" -le 2048
    printf 'present\n'
    # Actual reader cap even if the file grows after size checks. Host rejects
    # the 2049th byte sentinel; path/mode checks remain sequential, not atomic.
    dd if='@ROOT@/refusal.json' bs=2049 count=1 2>/dev/null
else
    test ! -e '@ROOT@/refusal.json'
    test ! -L '@ROOT@/refusal.json'
    printf 'absent\n'
fi
'@
}
function Save-FactsRefusalEvidence($Transport,[string]$Path,[string]$Nonce,[long]$AttemptPid,[string]$Start) {
    # No transport/effect here. Preserve bounded raw bytes even when decoding fails.
    if($Transport.timeout -or $Transport.exit -ne 0){throw 'Refusal collection transport unknown'}
    if($Transport.stdout -ceq "absent`n"){return [pscustomobject]@{state='absent';collected=$false;sha256=$null;decoded=$false;evidence=$null}}
    if($Transport.stdout -isnot [string] -or -not $Transport.stdout.StartsWith("present`n",[StringComparison]::Ordinal)){
        throw 'Refusal presence framing unknown'
    }
    $text=$Transport.stdout.Substring(8)
    if([Text.Encoding]::UTF8.GetByteCount($text) -gt 2048 -or [string]::IsNullOrEmpty($text)){throw 'Refusal bytes outside cap'}
    [IO.File]::WriteAllText($Path,$text,[Text.UTF8Encoding]::new($false))
    $value=ConvertFrom-FactsRefusal $text $Nonce $AttemptPid $Start
    return [pscustomobject]@{state='present';collected=$true;sha256=(Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant();decoded=($null -ne $value);evidence=$value}
}
