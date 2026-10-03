$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
. "$PSScriptRoot/facts-proof.ps1"
$nonce='0123456789abcdef0123456789abcdef'
$document='00000000-0000-4000-8000-000000000001'
$order=@(2..7 | ForEach-Object { '00000000-0000-4000-8000-{0:x12}' -f $_ })
$callback=[pscustomobject]@{nonce=$nonce;stage='facts-observed-no-change-during-read';application_thread=$true;engine_thread=$true}
$facts=[pscustomobject]@{kind='development-observed-facts';nonce=$nonce;attempt_pid='42';attempt_start='1234';required_metadata_validated=$true;required_connections_installed=$true;document_id=$document;current_page_id=$order[0];current_index=0;order=$order;instance='1';begin_epoch='0';end_epoch='0';begin_ms=1;end_ms=2;request_accepted_ms=100;delivered_ms=110;atomic_snapshot=$false;native_authority=$false;render_authority=$false}
$count=0
function Check([bool]$Condition,[string]$Name) { if (-not $Condition) {throw "FAIL: $Name"}; $script:count++ }
function Clone($Value) {return ($Value | ConvertTo-Json -Depth 10 -Compress | ConvertFrom-Json)}
Check (Test-FactsCallback $callback $nonce) 'valid callback'
Check (Test-FactsDiagnostics $facts $nonce $document $order 42 '1234') 'valid facts'
Check (-not (Test-FactsCallback $null $nonce)) 'null callback'
Check (-not (Test-FactsDiagnostics $null $nonce $document $order 42 '1234')) 'null facts'
foreach($stage in @('resolved','waiting-facts','open-observed','deadline','facts-entry-delivery-refused')) {
 $value=Clone $callback; $value.stage=$stage; Check (-not (Test-FactsCallback $value $nonce)) "stage $stage"
}
foreach($field in @('nonce','stage','application_thread','engine_thread')) {
 $value=Clone $callback; $value.PSObject.Properties.Remove($field); Check (-not (Test-FactsCallback $value $nonce)) "missing callback $field"
}
foreach($field in @('application_thread','engine_thread')) {
 foreach($bad in @($false,'true',1,$null)) { $value=Clone $callback; $value.$field=$bad; Check (-not (Test-FactsCallback $value $nonce)) "callback type $field" }
}
$value=Clone $callback; $value | Add-Member unexpected $true; Check (-not (Test-FactsCallback $value $nonce)) 'extra callback field'
$value=Clone $callback; $value.nonce='f'*32; Check (-not (Test-FactsCallback $value $nonce)) 'wrong nonce'
foreach($field in @($facts.PSObject.Properties.Name)) {
 $value=Clone $facts; $value.PSObject.Properties.Remove($field); Check (-not (Test-FactsDiagnostics $value $nonce $document $order 42 '1234')) "missing facts $field"
}
foreach($field in @('atomic_snapshot','native_authority','render_authority')) {
 foreach($bad in @($true,'false',0,$null)) { $value=Clone $facts; $value.$field=$bad; Check (-not (Test-FactsDiagnostics $value $nonce $document $order 42 '1234')) "authority/type $field" }
}
foreach($field in @('required_metadata_validated','required_connections_installed')) {
 $value=Clone $facts; $value.$field=$false; Check (-not (Test-FactsDiagnostics $value $nonce $document $order 42 '1234')) "metadata $field"
}
foreach($field in @('current_index','begin_ms','end_ms','request_accepted_ms','delivered_ms')) {
 $value=Clone $facts; $value.$field='1'; Check (-not (Test-FactsDiagnostics $value $nonce $document $order 42 '1234')) "integer type $field"
}
foreach($change in @(
 @{field='current_index';value=-1},@{field='current_index';value=6},@{field='current_page_id';value=$order[1]},
 @{field='begin_ms';value=-1},@{field='end_ms';value=0},@{field='end_ms';value=5000},
 @{field='request_accepted_ms';value=-1},@{field='request_accepted_ms';value=20000},
 @{field='delivered_ms';value=99},@{field='delivered_ms';value=5100},
 @{field='nonce';value=('f'*32)},@{field='attempt_pid';value='43'},@{field='attempt_start';value='1235'},
 @{field='instance';value='0'},@{field='instance';value='18446744073709551616'},@{field='begin_epoch';value='01'},@{field='end_epoch';value='1'},
 @{field='kind';value='native-snapshot'},@{field='document_id';value=$order[0]}
)) {
 $value=Clone $facts; $value.($change.field)=$change.value
 Check (-not (Test-FactsDiagnostics $value $nonce $document $order 42 '1234')) "invalid $($change.field)"
}
$value=Clone $facts; $value.order[0]=$value.order[1]; Check (-not (Test-FactsDiagnostics $value $nonce $document $order 42 '1234')) 'duplicate/order change'
$value=Clone $facts; $value.order=$order[0..4]; Check (-not (Test-FactsDiagnostics $value $nonce $document $order 42 '1234')) 'short order'
Check (-not (Test-FactsDiagnostics $facts $nonce $document $order 1 '1234')) 'invalid expected pid'
Check (-not (Test-FactsDiagnostics $facts $nonce $document $order 42 '01')) 'invalid expected start'
$value=Clone $facts; $value | Add-Member unexpected $true; Check (-not (Test-FactsDiagnostics $value $nonce $document $order 42 '1234')) 'extra private field'
Write-Output "facts validator checks: $count PASS"
