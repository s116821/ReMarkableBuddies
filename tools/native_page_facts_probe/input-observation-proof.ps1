# Source-only decoder. Completion proves a bounded GUI observation, never facts.
function Test-InputObservationLegacyOverviewPng([string]$path,[string]$profile) {
    if($profile -cne 'legacy-df745-overview-768x1024'){return $false}
    $file=Get-Item -LiteralPath $path
    if($file.Length -lt 45 -or $file.Length -gt 8388608){return $false}
    $stream=[IO.File]::OpenRead($path)
    try{
        # Fixed profile header/trailer sanity, not a full PNG decoder.
        $header=[byte[]]::new(33)
        if($stream.Read($header,0,33) -ne 33){return $false}
        if([Convert]::ToHexString($header[0..15]) -cne '89504E470D0A1A0A0000000D49484452'){return $false}
        $width=([long]$header[16]*16777216)+([long]$header[17]*65536)+([long]$header[18]*256)+$header[19]
        $height=([long]$header[20]*16777216)+([long]$header[21]*65536)+([long]$header[22]*256)+$header[23]
        if($width -ne 768 -or $height -ne 1024){return $false}
        $trailer=[byte[]]::new(12)
        [void]$stream.Seek(-12,[IO.SeekOrigin]::End)
        return $stream.Read($trailer,0,12) -eq 12 -and [Convert]::ToHexString($trailer) -ceq '0000000049454E44AE426082'
    }finally{$stream.Dispose()}
}
function Test-InputObservationCompletion($value,[string]$nonce,[string]$pidText,[string]$started,[string]$dev,[string]$ino) {
    if($null -eq $value -or $value.evidence_profile -cne 'device-frames-v1' -or $value.kind -cne 'development-input-observation' -or $value.nonce -cne $nonce){return $false}
    foreach($name in @('attempt_pid','attempt_start','root_device','root_inode')){
        if($value.$name -isnot [string] -or $value.$name -cnotmatch '^[1-9][0-9]{0,19}$'){return $false}
        $parsed=[ulong]0;if(-not [ulong]::TryParse($value.$name,[ref]$parsed)){return $false}
    }
    if($value.attempt_pid -cne $pidText -or $value.attempt_start -cne $started -or $value.root_device -cne $dev -or $value.root_inode -cne $ino){return $false}
    foreach($name in @('gui_callback_completed','application_thread','engine_thread','scope_current')){
        if($value.$name -isnot [bool] -or -not $value.$name){return $false}
    }
    foreach($name in @('native_authority','render_authority','ui_acknowledged')){
        if($value.$name -isnot [bool] -or $value.$name){return $false}
    }
    foreach($name in @('accepted_ms','seal_ms','grab_start_ms','grab_end_ms','completed_ms','width','height','image_width','image_height','png_bytes')){
        if(($value.$name -isnot [long] -and $value.$name -isnot [int]) -or $value.$name -lt 0){return $false}
    }
    if($value.accepted_ms -ge 120000 -or $value.seal_ms -ne $value.accepted_ms -or
       $value.grab_start_ms -lt $value.seal_ms -or $value.grab_end_ms -lt $value.grab_start_ms -or
       $value.completed_ms -lt $value.grab_end_ms -or $value.completed_ms -ge $value.accepted_ms+5000){return $false}
    if($value.width -le 0 -or $value.height -le 0 -or $value.dpr -isnot [ValueType] -or
       -not [double]::IsFinite([double]$value.dpr) -or $value.dpr -le 0 -or
       [double]$value.width*$value.height*$value.dpr*$value.dpr -gt 4194304){return $false}
    if($value.image_status -ceq 'available'){
        if($value.image_width -le 0 -or $value.image_height -le 0 -or
           [long]$value.image_width*$value.image_height -gt 4194304 -or $value.png_bytes -le 0 -or $value.png_bytes -gt 8388608){return $false}
    }elseif($value.image_status -ceq 'unsupported-empty'){
        if($value.image_width -ne 0 -or $value.image_height -ne 0 -or $value.png_bytes -ne 0){return $false}
    }else{return $false}
    if($value.counts -isnot [array] -or $value.counts.Count -ne 7 -or $value.events -isnot [array] -or $value.events.Count -gt 64){return $false}
    foreach($count in $value.counts){if(($count -isnot [long] -and $count -isnot [int]) -or $count -lt 0 -or $count -gt 4294967295){return $false}}
    foreach($name in @('record_overflow','point_overflow','count_overflow','output_truncated')){if($value.$name -isnot [bool]){return $false}}
    foreach($event in $value.events){
        foreach($name in @('device_present','device_system_id','device_type')){if($null -eq $event.PSObject.Properties[$name]){return $false}}
        if($event.device_present -isnot [bool]){return $false}
        if($event.device_present){
            if($event.device_system_id -isnot [string] -or $event.device_system_id -cnotmatch '\A(0|-?[1-9][0-9]{0,18})\z'){return $false}
            $deviceId=[long]0;if(-not [long]::TryParse($event.device_system_id,[ref]$deviceId)){return $false}
            if(($event.device_type -isnot [int] -and $event.device_type -isnot [long]) -or $event.device_type -lt 0 -or $event.device_type -gt 2147483647){return $false}
        }elseif($null -ne $event.device_system_id -or $null -ne $event.device_type){return $false}
        foreach($name in @('ms','type','relationship','source','buttons')){
            if(($event.$name -isnot [long] -and $event.$name -isnot [int]) -or $event.$name -lt 0){return $false}
        }
        if($event.ms -gt $value.seal_ms -or $event.type -notin @(2,3,5,194,195,196,209) -or
           $event.relationship -gt 3 -or $event.source -gt 3 -or $event.buttons -gt 2147483647 -or
           $event.timestamp -isnot [string] -or $event.timestamp -cnotmatch '^(0|[1-9][0-9]{0,19})$' -or
           $event.points -isnot [array] -or $event.points.Count -gt 4){return $false}
        $parsed=[ulong]0;if(-not [ulong]::TryParse($event.timestamp,[ref]$parsed)){return $false}
        foreach($point in $event.points){
            foreach($name in @('id','state')){
                if(($point.$name -isnot [int] -and $point.$name -isnot [long]) -or $point.$name -lt -2147483648 -or $point.$name -gt 2147483647){return $false}
            }
            if($null -eq $point.PSObject.Properties['valid_mask'] -or ($point.valid_mask -isnot [int] -and $point.valid_mask -isnot [long]) -or $point.valid_mask -lt 0 -or $point.valid_mask -gt 7){return $false}
            foreach($frame in @(@(1,'x','y'),@(2,'scene_x','scene_y'),@(4,'global_x','global_y'))){
                foreach($name in $frame[1..2]){
                    if($null -eq $point.PSObject.Properties[$name]){return $false}
                    if($point.valid_mask -band $frame[0]){
                        if(($point.$name -isnot [int] -and $point.$name -isnot [long] -and $point.$name -isnot [double]) -or -not [double]::IsFinite([double]$point.$name)){return $false}
                    }elseif($null -ne $point.$name){return $false}
                }
            }
        }
    }
    return $true
}
