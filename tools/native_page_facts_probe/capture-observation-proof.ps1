# Development correspondence only; this decoder grants no native/render authority.
function Test-CaptureObservationCompletion($value,[string]$nonce,[string]$pidText,[string]$started,[string]$dev,[string]$ino,[string]$document,[string[]]$order,[bool]$ReceiverSubtreeCapture=$false,[bool]$ReceiverSubtreeCapture512=$false,[int]$ReceiverSubtreeItemCap=0) {
    if($null -eq $value){return $false}
    if($ReceiverSubtreeItemCap -notin @(0,1024) -or ($ReceiverSubtreeItemCap -ne 0 -and (-not $ReceiverSubtreeCapture -or $ReceiverSubtreeCapture512))){return $false}
    if($ReceiverSubtreeCapture512 -and -not $ReceiverSubtreeCapture){return $false}
    $fields=@('kind','version','nonce','attempt_pid','attempt_start','root_device','root_inode','setup_profile','setup_budget_ms','capture_budget_ms','accepted_ms','baseline_ms','grab_start_ms','grab_end_ms','post_read_ms','completed_ms','document_id','page_id','page_index','begin_epoch','end_epoch','width','height','dpr','image_width','image_height','png_bytes','png_sha256','image_status','gui_callback_completed','scope_current','atomic_snapshot','native_authority','render_authority','ui_acknowledged','observed_order')
    if($ReceiverSubtreeCapture){
        $fields+='discovery_scope'
        if($value.discovery_scope -isnot [string] -or $value.discovery_scope -cne $(if($ReceiverSubtreeItemCap -eq 1024){'receiver-subtree-capture-unqualified-v3'}elseif($ReceiverSubtreeCapture512){'receiver-subtree-capture-unqualified-v2'}else{'receiver-subtree-capture-unqualified-v1'})){return $false}
    }
    $names=@($value.PSObject.Properties.Name)
    if($names.Count -ne $fields.Count -or @($names|Where-Object {$_ -cnotin $fields}).Count){return $false}
    foreach($name in @('kind','nonce','setup_profile','document_id','page_id','png_sha256','image_status')){if($value.$name -isnot [string]){return $false}}
    if($value.kind -cne 'development-capture-observation' -or $value.nonce -cne $nonce -or $nonce -cnotmatch '\A[0-9a-f]{32}\z' -or $value.setup_profile -cne 'main-dev-facts-120s' -or $value.image_status -cne 'available'){return $false}
    foreach($name in @('attempt_pid','attempt_start','root_device','root_inode','begin_epoch','end_epoch')){
        if($value.$name -isnot [string] -or $value.$name -cnotmatch '\A[1-9][0-9]{0,19}\z'){return $false}
        $parsed=[ulong]0;if(-not [ulong]::TryParse($value.$name,[ref]$parsed)){return $false}
    }
    if($value.attempt_pid -cne $pidText -or $value.attempt_start -cne $started -or $value.root_device -cne $dev -or $value.root_inode -cne $ino -or $value.begin_epoch -cne $value.end_epoch){return $false}
    foreach($name in @('version','setup_budget_ms','capture_budget_ms','accepted_ms','baseline_ms','grab_start_ms','grab_end_ms','post_read_ms','completed_ms','page_index','width','height','image_width','image_height','png_bytes')){
        if(($value.$name -isnot [int] -and $value.$name -isnot [long]) -or $value.$name -lt 0){return $false}
    }
    if($value.version -ne $(if($ReceiverSubtreeCapture){2}else{1}) -or $value.setup_budget_ms -ne 120000 -or $value.capture_budget_ms -ne 5000){return $false}
    $last=$value.accepted_ms
    foreach($name in @('baseline_ms','grab_start_ms','grab_end_ms','post_read_ms','completed_ms')){if($value.$name -lt $last){return $false};$last=$value.$name}
    if($value.accepted_ms -ge 120000 -or $value.completed_ms -ge 120000 -or $value.completed_ms -ge $value.accepted_ms+5000){return $false}
    $idPattern='\A[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\z'
    foreach($id in @($value.document_id,$value.page_id)){if($id -cnotmatch $idPattern -or $id -ceq '00000000-0000-0000-0000-000000000000'){return $false}}
    if($value.document_id -cne $document -or $order.Count -lt 1 -or $order.Count -gt 256 -or $value.page_index -ge $order.Count -or $value.page_id -cne $order[$value.page_index]){return $false}
    foreach($name in @('gui_callback_completed','scope_current')){if($value.$name -isnot [bool] -or -not $value.$name){return $false}}
    foreach($name in @('atomic_snapshot','native_authority','render_authority','ui_acknowledged','observed_order')){if($value.$name -isnot [bool] -or $value.$name){return $false}}
    if(($value.dpr -isnot [int] -and $value.dpr -isnot [long] -and $value.dpr -isnot [double]) -or -not [double]::IsFinite([double]$value.dpr) -or $value.dpr -le 0){return $false}
    if($value.width -le 0 -or $value.height -le 0 -or [double]$value.width*$value.height*$value.dpr*$value.dpr -gt 4194304 -or $value.image_width -le 0 -or $value.image_height -le 0 -or [double]$value.image_width*$value.image_height -gt 4194304 -or $value.png_bytes -lt 45 -or $value.png_bytes -gt 8388608 -or $value.png_sha256 -cnotmatch '\A[0-9a-f]{64}\z'){return $false}
    foreach($name in @('width','height','image_width','image_height')){if($value.$name -gt [int]::MaxValue){return $false}}
    if($value.image_width -ne [Math]::Floor([double]$value.width*$value.dpr+0.5) -or $value.image_height -ne [Math]::Floor([double]$value.height*$value.dpr+0.5)){return $false}
    return $true
}
function Test-CaptureObservationImage($value,[string]$path) {
    # Exact retrieved bytes plus PNG header/trailer sanity; visual review is separate.
    if(-not(Test-Path -LiteralPath $path -PathType Leaf)){return $false}
    $file=Get-Item -LiteralPath $path
    if($file.Length -ne $value.png_bytes -or $file.Length -lt 45 -or $file.Length -gt 8388608 -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $value.png_sha256){return $false}
    $stream=[IO.File]::OpenRead($path)
    try{
        $header=[byte[]]::new(33);$trailer=[byte[]]::new(12)
        if($stream.Read($header,0,33) -ne 33 -or [Convert]::ToHexString($header[0..15]) -cne '89504E470D0A1A0A0000000D49484452'){return $false}
        $width=([long]$header[16]*16777216)+([long]$header[17]*65536)+([long]$header[18]*256)+$header[19]
        $height=([long]$header[20]*16777216)+([long]$header[21]*65536)+([long]$header[22]*256)+$header[23]
        [void]$stream.Seek(-12,[IO.SeekOrigin]::End)
        return $width -eq $value.image_width -and $height -eq $value.image_height -and $stream.Read($trailer,0,12) -eq 12 -and [Convert]::ToHexString($trailer) -ceq '0000000049454E44AE426082'
    }finally{$stream.Dispose()}
}
