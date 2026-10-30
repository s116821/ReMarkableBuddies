# Separate unqualified diagnostic; no facts/admission/native/render success.
function Test-ReceiverSourceFactsCallback($Value,[string]$Nonce) {
    if($null -eq $Value -or $Nonce -cnotmatch '\A[0-9a-f]{32}\z'){return $false}
    return @($Value.PSObject.Properties).Count -eq 5 -and
        $Value.nonce -is [string] -and $Value.nonce -ceq $Nonce -and
        $Value.stage -is [string] -and $Value.stage -ceq 'receiver-source-facts-observed-unqualified' -and
        $Value.application_thread -is [bool] -and $Value.application_thread -and
        $Value.engine_thread -is [bool] -and $Value.engine_thread -and
        $Value.source_facts_reader_stage -is [string] -and $Value.source_facts_reader_stage -ceq 'facts-observed-no-change-during-read'
}
function ConvertFrom-ReceiverSourceFactsRaw([string]$Raw) {
    if([Text.Encoding]::UTF8.GetByteCount($Raw) -gt 8192){return $null}
    $document=$null
    try {
        $document=[System.Text.Json.JsonDocument]::Parse($Raw)
        if($document.RootElement.ValueKind -ne [System.Text.Json.JsonValueKind]::Object){return $null}
        $names=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach($property in $document.RootElement.EnumerateObject()){if(-not $names.Add($property.Name)){return $null}}
        return ConvertFrom-Json -InputObject $Raw -ErrorAction Stop
    } catch {return $null} finally {if($null -ne $document){$document.Dispose()}}
}
function Test-ReceiverSourceFacts($Value,[string]$Nonce,$Identity,$Expected) {
    if($null -eq $Value){return $false}
    $fields=@('kind','version','scope','nonce','attempt_pid','attempt_start','root_device','root_inode','document_id','page_id','page_index','page_count','order','alias_matches','forward_reverse_mapping_matches','observed_order','accepted_ms','read_begin_ms','read_end_ms','effective_deadline_ms','begin_epoch','end_epoch','atomic_snapshot','native_authority','render_authority','ui_acknowledged','delivered_ms')
    $names=@($Value.PSObject.Properties.Name)
    if($names.Count -ne 27 -or @($names|Where-Object {$_ -cnotin $fields}).Count){return $false}
    if($Value.kind -isnot [string] -or $Value.kind -cne 'development-receiver-source-facts' -or $Value.scope -isnot [string] -or $Value.scope -cne 'receiver-source-facts-unqualified-v1' -or ($Value.version -isnot [int] -and $Value.version -isnot [long]) -or $Value.version -ne 1){return $false}
    if($Value.nonce -isnot [string] -or $Value.nonce -cne $Nonce -or $Nonce -cnotmatch '\A[0-9a-f]{32}\z'){return $false}
    foreach($name in @('attempt_pid','attempt_start','root_device','root_inode')){
        $number=0UL
        if($Value.$name -isnot [string] -or $Value.$name -cnotmatch '\A[1-9][0-9]{0,19}\z' -or -not [ulong]::TryParse($Value.$name,[ref]$number) -or $Value.$name -cne $Identity.$name){return $false}
    }
    $uuid='\A[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\z'
    $nil='00000000-0000-0000-0000-000000000000'
    if($Value.document_id -isnot [string] -or $Value.document_id -cnotmatch $uuid -or $Value.document_id -ceq $nil -or $Value.document_id -cne $Expected.document -or $Value.order -isnot [array] -or $Value.order.Count -ne 6 -or $Expected.order -isnot [array] -or $Expected.order.Count -ne 6 -or @($Value.order|Select-Object -Unique).Count -ne 6){return $false}
    for($index=0;$index -lt 6;$index++){if($Value.order[$index] -isnot [string] -or $Value.order[$index] -cnotmatch $uuid -or $Value.order[$index] -ceq $nil -or $Value.order[$index] -cne $Expected.order[$index]){return $false}}
    foreach($name in @('page_index','page_count','accepted_ms','read_begin_ms','read_end_ms','effective_deadline_ms','delivered_ms')){if(($Value.$name -isnot [int] -and $Value.$name -isnot [long]) -or $Value.$name -lt 0 -or $Value.$name -gt 2147483647){return $false}}
    if($Value.page_count -ne 6 -or $Value.page_index -ge 6 -or $Value.page_id -isnot [string] -or $Value.page_id -cne $Value.order[$Value.page_index]){return $false}
    if($Value.accepted_ms -ge 120000 -or $Value.read_begin_ms -lt $Value.accepted_ms -or $Value.read_end_ms -lt $Value.read_begin_ms -or $Value.delivered_ms -lt $Value.read_end_ms -or $Value.delivered_ms -ge $Value.effective_deadline_ms -or $Value.effective_deadline_ms -ne [Math]::Min(120000,$Value.accepted_ms+5000)){return $false}
    foreach($name in @('alias_matches','forward_reverse_mapping_matches','observed_order')){if($Value.$name -isnot [bool] -or -not $Value.$name){return $false}}
    foreach($name in @('atomic_snapshot','native_authority','render_authority','ui_acknowledged')){if($Value.$name -isnot [bool] -or $Value.$name){return $false}}
    foreach($name in @('begin_epoch','end_epoch')){$number=0UL;if($Value.$name -isnot [string] -or $Value.$name -cnotmatch '\A(0|[1-9][0-9]{0,19})\z' -or -not [ulong]::TryParse($Value.$name,[ref]$number)){return $false}}
    return $Value.begin_epoch -ceq $Value.end_epoch
}
function Get-ReceiverSourceFactsReadCommand {
    return @'
set -eu
test -d '@ROOT@' && test ! -L '@ROOT@'
test "$(stat -c '%a %u' '@ROOT@')" = '700 0'
test -f '@ROOT@/owner' && test ! -L '@ROOT@/owner'
test "$(stat -c '%a %u' '@ROOT@/owner')" = '600 0'
test "$(cat '@ROOT@/owner')" = '@NONCE@'
if test ! -e '@ROOT@/receiver-source-facts.json' && test ! -L '@ROOT@/receiver-source-facts.json'; then exit 3; fi
test -f '@ROOT@/receiver-source-facts.json' && test ! -L '@ROOT@/receiver-source-facts.json'
test "$(stat -c '%a %u' '@ROOT@/receiver-source-facts.json')" = '600 0'
test "$(wc -c < '@ROOT@/receiver-source-facts.json')" -le 8192
cat '@ROOT@/receiver-source-facts.json'
'@
}
function Save-ReceiverSourceFactsRaw($Transport,[string]$Path) {
    if($Transport.timeout -or $Transport.exit -ne 0){return $null}
    $bytes=[Text.Encoding]::UTF8.GetBytes([string]$Transport.stdout)
    if($bytes.Length -gt 8192 -or (Test-Path -LiteralPath $Path)){throw 'Source facts saved bytes refused'}
    [IO.File]::WriteAllBytes($Path,$bytes)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}
