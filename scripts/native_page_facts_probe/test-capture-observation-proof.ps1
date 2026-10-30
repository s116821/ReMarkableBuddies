$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-observation-proof.ps1"
$nonce='0123456789abcdef0123456789abcdef';$document='11111111-1111-1111-1111-111111111111';$page='22222222-2222-2222-2222-222222222222'
$path=Join-Path ([IO.Path]::GetTempPath()) ('capture-proof-'+[Guid]::NewGuid().ToString('N')+'.png')
$png=[Convert]::FromBase64String('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a9d8AAAAASUVORK5CYII=')
[IO.File]::WriteAllBytes($path,$png);$sha=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
$count=0
function Check([bool]$ok,[string]$name){if(-not $ok){throw "FAIL $name"};$script:count++}
function New-Completion {
    [pscustomobject]@{kind='development-capture-observation';version=1;nonce=$nonce;attempt_pid='1234';attempt_start='5678';root_device='11';root_inode='22';setup_profile='main-dev-facts-120s';setup_budget_ms=120000;capture_budget_ms=5000;
    accepted_ms=100;baseline_ms=101;grab_start_ms=102;grab_end_ms=103;post_read_ms=104;completed_ms=105;document_id=$document;page_id=$page;page_index=0;begin_epoch='1';end_epoch='1';width=1;height=1;dpr=1.0;image_width=1;image_height=1;png_bytes=$png.Length;png_sha256=$sha;image_status='available';gui_callback_completed=$true;scope_current=$true;atomic_snapshot=$false;native_authority=$false;render_authority=$false;ui_acknowledged=$false;observed_order=$false}
}
function Valid($value){Test-CaptureObservationCompletion $value $nonce '1234' '5678' '11' '22' $document @($page)}
try{
    $good=New-Completion;Check (Valid $good) 'valid';Check (Test-CaptureObservationImage $good $path) 'exact PNG binding'
    foreach($field in @($good.PSObject.Properties.Name)){
        $bad=New-Completion;$bad.PSObject.Properties.Remove($field);Check (-not(Valid $bad)) "missing $field"
        $bad=New-Completion;$bad.$field=$null;Check (-not(Valid $bad)) "null $field"
    }
    $bad=New-Completion;$bad|Add-Member extra 1;Check (-not(Valid $bad)) 'extra field'
    foreach($field in @('atomic_snapshot','native_authority','render_authority','ui_acknowledged','observed_order')){$bad=New-Completion;$bad.$field=$true;Check (-not(Valid $bad)) "authority $field"}
    foreach($field in @('gui_callback_completed','scope_current')){$bad=New-Completion;$bad.$field=$false;Check (-not(Valid $bad)) "false $field"}
    foreach($field in @('attempt_pid','attempt_start','root_device','root_inode','begin_epoch','end_epoch')){
        foreach($wrong in @('0','01','18446744073709551616',1)){$bad=New-Completion;$bad.$field=$wrong;Check (-not(Valid $bad)) "identity $field $wrong"}
    }
    foreach($field in @('version','setup_budget_ms','capture_budget_ms','accepted_ms','baseline_ms','grab_start_ms','grab_end_ms','post_read_ms','completed_ms','page_index','width','height','image_width','image_height','png_bytes')){
        $bad=New-Completion;$bad.$field='1';Check (-not(Valid $bad)) "string integer $field"
        $bad=New-Completion;$bad.$field=1.5;Check (-not(Valid $bad)) "float integer $field"
    }
    $bad=New-Completion;$bad.completed_ms=5100;Check (-not(Valid $bad)) 'capture exact deadline'
    $bad=New-Completion;$bad.accepted_ms=119995;$bad.baseline_ms=119996;$bad.grab_start_ms=119997;$bad.grab_end_ms=119998;$bad.post_read_ms=119999;$bad.completed_ms=120000;Check (-not(Valid $bad)) 'original setup exact deadline'
    $bad.completed_ms=119999;Check (Valid $bad) 'remaining original setup'
    $bad=New-Completion;$bad.post_read_ms=102;Check (-not(Valid $bad)) 'backwards post read'
    $bad=New-Completion;$bad.end_epoch='2';Check (-not(Valid $bad)) 'sticky epoch mismatch'
    $bad=New-Completion;$bad.document_id=$page;Check (-not(Valid $bad)) 'wrong document'
    $bad=New-Completion;$bad.page_id=$document;Check (-not(Valid $bad)) 'wrong expected page'
    $bad=New-Completion;$bad.page_index=1;Check (-not(Valid $bad)) 'index outside order'
    foreach($wrong in @([double]::NaN,[double]::PositiveInfinity,0,'1')){$bad=New-Completion;$bad.dpr=$wrong;Check (-not(Valid $bad)) 'DPR invalid'}
    $bad=New-Completion;$bad.width=4194305;Check (-not(Valid $bad)) 'window pixel cap'
    $bad=New-Completion;$bad.image_height=4194305;Check (-not(Valid $bad)) 'image pixel cap'
    $bad=New-Completion;$bad.image_width=2;Check (-not(Valid $bad)) 'window image mismatch'
    $bad=New-Completion;$bad.png_bytes=8388609;Check (-not(Valid $bad)) 'encoded cap'
    $bad=New-Completion;$bad.png_sha256=('a'*64);Check (-not(Test-CaptureObservationImage $bad $path)) 'digest mismatch'
    $bad=New-Completion;$bad.png_bytes++;Check (-not(Test-CaptureObservationImage $bad $path)) 'byte mismatch'
    $bad=New-Completion;$bad.image_width=2;Check (-not(Test-CaptureObservationImage $bad $path)) 'dimension mismatch'
    $bad=New-Completion;$bad.setup_profile='main-dev-input-observation-120s';Check (-not(Valid $bad)) 'cross purpose profile'
    Write-Output "PASS capture observation proof: $count checks (host fixture only)"
}finally{Remove-Item -LiteralPath $path -Force}
