param([string]$CxxRefusalPath)
$ErrorActionPreference='Stop'
. "$PSScriptRoot/facts-refusal-proof.ps1"
. "$PSScriptRoot/facts-proof.ps1"
$nonce='0123456789abcdef0123456789abcdef'
$count=0
function New-Refusal {
    [pscustomobject]@{kind='development-facts-refusal';version=1;nonce=$nonce;attempt_pid='42';attempt_start='1234';
      reader_stage='facts-values-refused';reader_result_had_facts=$false;entry_stage='facts-entry-read-refused';refusal_path='reader-result';
      sample_phase='before-refusal-callback';sample_ms=120010;request_accepted_ms=119999;accepted_read_elapsed_ms=11;
      setup_clock_origin='entry-startup-monotonic';read_clock_origin='accepted-request-monotonic';setup_budget_ms=120000;read_budget_ms=5000;
      setup_selection='main-dev-facts-120s';development_setup_opt_in=$true;entry_context_current=$true;root_identity_current=$true;
      closure_absent=$true;attempt_identity_current=$true;within_accepted_deadline=$true;atomic_snapshot=$false;native_authority=$false;render_authority=$false}
}
function Check([bool]$value,[string]$name){if(-not $value){throw "FAIL $name"};$script:count++}
function Decode($value){ConvertFrom-FactsRefusal ($value|ConvertTo-Json -Depth 8 -Compress) $nonce 42 '1234'}
function Bad($value,[string]$name){Check ($null -eq (Decode $value)) $name}
Check ($null -ne (Decode (New-Refusal))) 'valid27'
foreach($stage in Get-FactsRefusalStages){$r=New-Refusal;$r.reader_stage=$stage;Check ($null -ne (Decode $r)) "allowed $stage"}
foreach($field in (New-Refusal).PSObject.Properties.Name){$r=New-Refusal;$r.PSObject.Properties.Remove($field);Bad $r "missing $field"}
$r=New-Refusal;$r|Add-Member extra $true;Bad $r 'extra field'
foreach($field in @('version','sample_ms','request_accepted_ms','accepted_read_elapsed_ms','setup_budget_ms','read_budget_ms')){
    foreach($bad in @('1',1.5,$null)){$r=New-Refusal;$r.$field=$bad;Bad $r "number type $field"}
}
foreach($field in @('reader_result_had_facts','development_setup_opt_in','entry_context_current','root_identity_current','closure_absent',
   'attempt_identity_current','within_accepted_deadline','atomic_snapshot','native_authority','render_authority')){
    foreach($bad in @('false',0,$null)){$r=New-Refusal;$r.$field=$bad;Bad $r "bool type $field"}
}
foreach($change in @(@{f='kind';v='development-observed-facts'},@{f='version';v=2},@{f='nonce';v=('f'*32)},@{f='attempt_pid';v='43'},
    @{f='attempt_start';v='1235'},@{f='reader_stage';v='raw QML error 0x12345678'},@{f='entry_stage';v='facts-observed-no-change-during-read'},
    @{f='refusal_path';v='entry-read-boundary'},@{f='sample_phase';v='after-callback'},@{f='setup_clock_origin';v='reset'},
    @{f='read_clock_origin';v='reset'},@{f='setup_budget_ms';v=20000},@{f='read_budget_ms';v=5001},@{f='setup_selection';v='default-dev-20s'},
    @{f='development_setup_opt_in';v=$false},@{f='sample_ms';v=119998},@{f='request_accepted_ms';v=120000},@{f='accepted_read_elapsed_ms';v=12},
    @{f='within_accepted_deadline';v=$false},@{f='atomic_snapshot';v=$true},@{f='native_authority';v=$true},@{f='render_authority';v=$true})){
    $r=New-Refusal;$r.($change.f)=$change.v;Bad $r "wrong $($change.f)"
}
$r=New-Refusal;$r.reader_result_had_facts=$true;$r.refusal_path='entry-read-boundary';Check ($null -ne (Decode $r)) 'entry read boundary'
$r.entry_stage='facts-entry-delivery-refused';$r.refusal_path='entry-completion-boundary';$r.sample_ms=124999;$r.accepted_read_elapsed_ms=5000;$r.within_accepted_deadline=$false
Check ($null -ne (Decode $r)) 'expired diagnostic valid but no facts authority'
$r.entry_context_current=$false;Check ($null -ne (Decode $r)) 'context-loss diagnostic'
$r.reader_result_had_facts=$false;Bad $r 'completion contradicts had-facts'
$r=New-Refusal;$r.sample_ms=-1;$r.request_accepted_ms=-1;$r.accepted_read_elapsed_ms=-1;$r.within_accepted_deadline=$false;Check ($null -ne (Decode $r)) 'unknown-clock sentinel'
$r.within_accepted_deadline=$true;Bad $r 'unknown cannot claim deadline'
$text=(New-Refusal)|ConvertTo-Json -Compress
foreach($invalid in @('', 'null', '[]', '{', ($text.Replace('"kind":','"kind":"development-facts-refusal","kind":')),($text.Replace('"kind"','"Kind"')),(' '*2049))){
    Check ($null -eq (ConvertFrom-FactsRefusal $invalid $nonce 42 '1234')) 'raw shape/duplicate/cap refuses'
}
$order=@(2..7|ForEach-Object {'00000000-0000-4000-8000-{0:x12}' -f $_})
$decoded=Decode (New-Refusal)
Check (-not(Test-FactsDiagnostics $decoded $nonce '00000000-0000-4000-8000-000000000001' $order 42 '1234')) 'never success private'
$cb=[pscustomobject]@{nonce=$nonce;stage='facts-entry-read-refused';application_thread=$true;engine_thread=$false}
Check (Test-FactsRefusalCallback $cb $nonce) 'typed refusal callback permits lost context'
Check (-not(Test-FactsCallback $cb $nonce)) 'never success callback'
$cb|Add-Member extra $true;Check (-not(Test-FactsRefusalCallback $cb $nonce)) 'extra callback refuses'
if($CxxRefusalPath){
    $actual=Get-Content -LiteralPath $CxxRefusalPath -Raw
    Check ($null -ne (ConvertFrom-FactsRefusal $actual $nonce 1234 '5678')) 'actual C++ owned record'
}
$directory=Join-Path ([IO.Path]::GetTempPath()) ('refusal-decoder-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory $directory)
try{
    $path=Join-Path $directory 'refusal.json'
    foreach($case in @(@{text="present`n"+$text;decoded=$true},@{text="present`n"+'{';decoded=$false},@{text="absent`n";decoded=$false})){
        $saved=Save-FactsRefusalEvidence @{timeout=$false;exit=0;stdout=$case.text} $path $nonce 42 '1234'
        Check ($saved.decoded -eq $case.decoded) 'bounded save diagnostic only'
    }
    foreach($transport in @(@{timeout=$true;exit=0;stdout=''},@{timeout=$false;exit=90;stdout=''},
       @{timeout=$false;exit=0;stdout='bad'},@{timeout=$false;exit=0;stdout="present`n"+('x'*2049)},
       @{timeout=$false;exit=0;stdout="present`n"+([char]0x20ac).ToString()*1000})){
        $threw=$false;try{[void](Save-FactsRefusalEvidence $transport $path $nonce 42 '1234')}catch{$threw=$true}
        Check $threw 'transport/framing/UTF8 byte cap refuses'
    }
    $future=Join-Path $PSScriptRoot 'run-qt-page-facts-diagnostic-source.ps1'
    $blocked=$false;try{& $future -PayloadPath absent -PublisherPath absent -ExpectedPath absent -StockBaselinePath absent -EvidenceDirectory (Join-Path $directory 'unused') -PrepareOnly}catch{$blocked=$_.Exception.Message.StartsWith('SOURCE ONLY:',[StringComparison]::Ordinal)}
    Check $blocked 'future template cannot execute/prepare spent literals'
    Check (-not(Test-Path (Join-Path $directory 'unused'))) 'guard precedes filesystem/transport'
}finally{Remove-Item -LiteralPath $directory -Recurse -Force}
Write-Output "PASS $count fixed-refusal decode/authority/bounded-save/source-guard checks"
