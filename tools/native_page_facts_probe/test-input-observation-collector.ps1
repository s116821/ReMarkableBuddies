$ErrorActionPreference='Stop'
. "$PSScriptRoot/input-observation-proof.ps1"
. "$PSScriptRoot/development-budget.ps1"
$source=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-input-observation-source.ps1'))
$tokens=$null;$errors=$null;$ast=[Management.Automation.Language.Parser]::ParseInput($source,[ref]$tokens,[ref]$errors)
if($errors.Count){throw ($errors|Out-String)}
$functions=$ast.FindAll({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst]},$true)
foreach($name in @('Require','RequireSameObservationIdentity')){. ([scriptblock]::Create(($functions|Where-Object Name -ceq $name).Extent.Text))}
$begin=$source.IndexOf('            $identityCommand=Expand @''');$end=$source.IndexOf('            break',$begin)
$branch=[scriptblock]::Create($source.Substring($begin,$end-$begin))
$nonce='0123456789abcdef0123456789abcdef';$remote='/run/rmb-qt-probe-'+$nonce
$packet=Join-Path ([IO.Path]::GetTempPath()) ('input-observation-test-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $packet)
$count=0
function Check([bool]$ok,[string]$label){if(-not $ok){throw "FAIL $label"};$script:count++}
function New-Completion {
    [pscustomobject]@{kind='development-input-observation';evidence_profile='device-frames-v1';nonce=$nonce;attempt_pid='1234';attempt_start='5678';root_device='11';root_inode='22';
        gui_callback_completed=$true;application_thread=$true;engine_thread=$true;scope_current=$true;native_authority=$false;render_authority=$false;ui_acknowledged=$false;
        accepted_ms=100;seal_ms=100;grab_start_ms=101;grab_end_ms=102;completed_ms=103;width=200;height=200;dpr=1;
        image_status='available';image_width=200;image_height=200;png_bytes=$png.Length;counts=@(0,0,0,0,1,0,0);events=@();
        record_overflow=$false;point_overflow=$false;count_overflow=$false;output_truncated=$false}
}
function Expand([string]$text){$text.Replace('@ROOT@',$remote).Replace('@NONCE@',$nonce)}
function Hash([string]$path){(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}
$png=[Convert]::FromBase64String('iVBORw0KGgoAAAANSUhEUgAAAwAAAAQAAQAAAAB+XQOtAAACpElEQVR4nO3SwQkAIBAEMbX/nrWEeYgPISngDoade7y1Ht8fHiSJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCRKEiWJkkRJoiRRkihJlCSS6J4VJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJEoSJYmSREmiJFGSKEmUJJLonhUlidL/iQ5wrQj/iA+lSgAAAABJRU5ErkJggg==')
$pngPath=Join-Path $packet 'fixture-legacy-overview.png';[IO.File]::WriteAllBytes($pngPath,$png);$pngHash=Hash $pngPath
function ObservationSSH([string]$command){
    if($command.Contains('/home/root/rem9-validation/screenshot')){
        $script:captures++
        if($case -ceq 'heap-timeout'){return @{exit=1;timeout=$true;stdout='partial'}}
        if($case -ceq 'heap-disconnect'){return @{exit=255;timeout=$false;stdout='partial'}}
        if($case -ceq 'heap-malformed'){return @{exit=0;timeout=$false;stdout='partial'}}
        return @{exit=0;timeout=$false;stdout=($pngHash+'  /tmp/rem25-facts-'+$nonce+"-input-observation.png`n")}
    }
    if($command.Contains("sha256sum '$remote/input-window.png'")){return @{exit=0;timeout=$false;stdout=($pngHash+'  '+$remote+"/input-window.png`n")}}
    $script:identities++
    if($case -ceq 'initial-generation-loss' -or ($case -ceq 'late-generation-loss' -and $captures -gt 0) -or ($case -ceq 'qt-postcopy-generation-loss' -and $identities -ge 3)){
        return @{exit=0;timeout=$false;stdout="1234 9999 11 22`n"}
    }
    return @{exit=0;timeout=$false;stdout="1234 5678 11 22`n"}
}
function ObservationCopy([string]$remotePath,[string]$localPath){
    if($case -ceq 'qt-copy-timeout' -and $remotePath.StartsWith('/run/')){return @{exit=1;timeout=$true;stdout=''}}
    if($case -ceq 'heap-copy-timeout' -and $remotePath.StartsWith('/tmp/')){return @{exit=1;timeout=$true;stdout=''}}
    [IO.File]::WriteAllBytes($localPath,$(if($case -ceq 'heap-hash-mismatch' -and $remotePath.StartsWith('/tmp/')){[byte[]]@(0)}else{$png}));return @{exit=0;timeout=$false;stdout=''}
}
try{
    Check ($source.Contains("heap_image_profile='legacy-df745-overview-768x1024';capture_helper_sha256='df745")) 'packet fixed profile/helper binding'
    Check ($source.Contains("experiment='development-input-observation';evidence_profile='device-frames-v1';heap_image_profile='legacy-df745-overview-768x1024'")) 'receipt fixed profile binding'
    Check ($source.Contains((Hash (Join-Path $PSScriptRoot 'input-observation-proof.ps1')))) 'frozen observation proof hash'
    $completion=New-Completion
    Check (Test-InputObservationCompletion $completion $nonce '1234' '5678' '11' '22') 'valid completion'
    foreach($field in @('native_authority','render_authority','ui_acknowledged')){
        $bad=New-Completion;$bad.$field=$true
        Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) $field
    }
    foreach($field in @('attempt_pid','attempt_start','root_device','root_inode')){
        foreach($value in @('0','01234','184467440737095516160')){
            $bad=New-Completion;$bad.$field=$value
            Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) "$field $value"
        }
    }
    foreach($field in @('accepted_ms','seal_ms','grab_start_ms','grab_end_ms','completed_ms')){
        $bad=New-Completion;$bad.$field='100'
        Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) "$field type"
    }
    $bad=New-Completion;$bad.completed_ms=5100
    Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) 'deadline exact'
    $bad=New-Completion;$bad.png_bytes=8388609
    Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) 'PNG cap'
    Check (Test-InputObservationLegacyOverviewPng $pngPath 'legacy-df745-overview-768x1024') 'fixed legacy overview'
    foreach($profile in @('', 'native-1404x1872', 'LEGACY-df745-overview-768x1024')){
        Check (-not(Test-InputObservationLegacyOverviewPng $pngPath $profile)) 'wrong profile refused'
    }
    $badPngPath=Join-Path $packet 'refused.png'
    foreach($shape in @(@(1404,1872),@(1024,768),@(767,1024))){
        $bytes=$png.Clone()
        foreach($pair in @(@(16,$shape[0]),@(20,$shape[1]))){
            $encoded=[BitConverter]::GetBytes([int]$pair[1]);[Array]::Reverse($encoded);[Array]::Copy($encoded,0,$bytes,$pair[0],4)
        }
        [IO.File]::WriteAllBytes($badPngPath,$bytes)
        Check (-not(Test-InputObservationLegacyOverviewPng $badPngPath 'legacy-df745-overview-768x1024')) 'wrong dimensions refused'
    }
    foreach($kind in @('signature','truncated','trailer','oversize')){
        $bytes=$png.Clone()
        switch($kind){
            signature {$bytes[0]=0}
            truncated {$bytes=$bytes[0..23]}
            trailer {$bytes[-1]=0}
            oversize {$bytes=[byte[]]::new(8388609)}
        }
        [IO.File]::WriteAllBytes($badPngPath,$bytes)
        Check (-not(Test-InputObservationLegacyOverviewPng $badPngPath 'legacy-df745-overview-768x1024')) "$kind PNG refused"
    }
    foreach($profile in @($null,'','old','DEVICE-frames-v1')){
        $bad=New-Completion;$bad.evidence_profile=$profile
        Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) 'wrong/missing evidence profile'
    }
    foreach($profileJson in @('[]','["device-frames-v1"]','["device-frames-v1","other"]','true','1','{}')){
        $bad=New-Completion;$bad.evidence_profile=ConvertFrom-Json -InputObject $profileJson -NoEnumerate
        $bad=($bad|ConvertTo-Json -Depth 10 -Compress)|ConvertFrom-Json
        Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) "profile JSON type refused $profileJson"
    }
    Check ($source.Contains("evidence_profile='device-frames-v1';heap_image_profile=")) 'fixed evidence profile binding'
    function New-FrameCompletion([int]$mask=7){
        $v=New-Completion
        $point=[pscustomobject]@{id=0;state=1;valid_mask=$mask;x=$null;y=$null;scene_x=$null;scene_y=$null;global_x=$null;global_y=$null}
        foreach($frame in @(@(1,'x','y'),@(2,'scene_x','scene_y'),@(4,'global_x','global_y'))){if($mask -band $frame[0]){$point.($frame[1])=[double]::MaxValue;$point.($frame[2])=-8388608.0}}
        $v.events=@([pscustomobject]@{device_present=$true;device_system_id='0';device_type=0;ms=99;type=2;relationship=1;source=0;buttons=1;timestamp='0';points=@($point)})
        return $v
    }
    foreach($mask in 0..7){
        $v=New-FrameCompletion $mask
        Check (Test-InputObservationCompletion $v $nonce '1234' '5678' '11' '22') "valid frame mask $mask"
        foreach($name in @('x','y','scene_x','scene_y','global_x','global_y')){
            $bad=New-FrameCompletion $mask;$bad.events[0].points[0].PSObject.Properties.Remove($name)
            Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) "$mask missing $name"
        }
        foreach($frame in @(@(1,'x','y'),@(2,'scene_x','scene_y'),@(4,'global_x','global_y'))){
            $bad=New-FrameCompletion $mask;$bad.events[0].points[0].($frame[1])=$(if($mask -band $frame[0]){$null}else{0.0})
            Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) "$mask contradictory frame"
        }
    }
    foreach($sid in @('-9223372036854775808','9223372036854775807','-1','0','1')){
        $v=New-FrameCompletion;$v.events[0].device_system_id=$sid
        $v=($v|ConvertTo-Json -Depth 10 -Compress)|ConvertFrom-Json
        Check ((Test-InputObservationCompletion $v $nonce '1234' '5678' '11' '22') -and $v.events[0].device_system_id -ceq $sid) 'signed device exact string roundtrip'
    }
    foreach($sid in @('-9223372036854775809','9223372036854775808','-0','+1','01','-01','1.0','',"1`n","1`r`n","1`r",' 1','1 ',"1`t","`t1",1,$null)){
        $bad=New-FrameCompletion;$bad.events[0].device_system_id=$sid
        Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) 'device ID refused'
    }
    foreach($name in @('device_present','device_system_id','device_type')){
        $bad=New-FrameCompletion;$bad.events[0].PSObject.Properties.Remove($name)
        Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) "missing $name"
    }
    foreach($type in @(-1,2147483648,'1',$true,1.5,$null)){
        $bad=New-FrameCompletion;$bad.events[0].device_type=$type
        Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) 'device type refused'
    }
    $v=New-FrameCompletion;$v.events[0].device_present=$false;$v.events[0].device_system_id=$null;$v.events[0].device_type=$null
    Check (Test-InputObservationCompletion $v $nonce '1234' '5678' '11' '22') 'null device explicit null'
    foreach($name in @('device_system_id','device_type')){$bad=New-FrameCompletion;$bad.events[0].device_present=$false;$bad.events[0].device_system_id=$null;$bad.events[0].device_type=$null;$bad.events[0].$name=0;Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) 'false device nonnull refused'}
    foreach($present in @('true',1,$null)){$bad=New-FrameCompletion;$bad.events[0].device_present=$present;Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) 'device present bool required'}
    $v=New-FrameCompletion;$v.events[0].device_type=2147483647
    Check (Test-InputObservationCompletion $v $nonce '1234' '5678' '11' '22') 'unknown bounded numeric device type retained'
    $bad=New-Completion;$bad.PSObject.Properties.Remove('evidence_profile')
    Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) 'old payload without profile refused'
    foreach($value in @('7',$true,-1,8,$null)){$bad=New-FrameCompletion;$bad.events[0].points[0].valid_mask=$value;Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) 'mask type/range refused'}
    foreach($name in @('x','y','scene_x','scene_y','global_x','global_y')){
        foreach($invalid in @([double]::NaN,[double]::PositiveInfinity,[double]::NegativeInfinity,'1',$true,$null)){
            $bad=New-FrameCompletion;$bad.events[0].points[0].$name=$invalid
            Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) "nonfinite/nonnumber $name refused"
        }
    }
    $event=[pscustomobject]@{device_present=$false;device_system_id=$null;device_type=$null;ms=99;type=2;relationship=1;source=0;buttons=1;timestamp='0';points=@([pscustomobject]@{id=0;state=1;valid_mask=7;x=90.0;y=260.0;scene_x=190.0;scene_y=360.0;global_x=290.0;global_y=460.0})}
    $good=New-Completion;$good.events=@($event)
    Check (Test-InputObservationCompletion $good $nonce '1234' '5678' '11' '22') 'typed event coordinates'
    foreach($field in @('ms','type','relationship','source','buttons')){
        $bad=New-Completion;$bad.events=@([pscustomobject]@{device_present=$false;device_system_id=$null;device_type=$null;ms=99;type=2;relationship=1;source=0;buttons=1;timestamp='0';points=@()});$bad.events[0].$field='1'
        Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) "$field event type"
    }
    $bad=New-Completion;$bad.events=@($event);$event.points=@(1,2,3,4,5)
    Check (-not(Test-InputObservationCompletion $bad $nonce '1234' '5678' '11' '22')) 'point cap'
    foreach($case in @('good','empty-image','qt-copy-timeout','qt-postcopy-generation-loss','heap-timeout','heap-disconnect','heap-malformed','late-generation-loss','initial-generation-loss','heap-copy-timeout','heap-hash-mismatch')){
        $record=[ordered]@{facts_verified=$false;callback_verified=$false;candidate_generation_verified=$false;gui_completion_verified=$false;qt_image_available=$false;qt_saved_copy_verified=$false;heap_saved_copy_verified=$false;heap_capture_succeeded=$false;paired_image_candidate_verified=$false}
        $captures=0;$identities=0;$observationClock=[pscustomobject]@{ElapsedMilliseconds=120}
        $value=New-Completion
        if($case -ceq 'empty-image'){$value.image_status='unsupported-empty';$value.image_width=0;$value.image_height=0;$value.png_bytes=0}
        $observed=@{stdout=($value|ConvertTo-Json -Depth 8 -Compress)}
        $failed=$false;try{. $branch}catch{$failed=$true}
        Check (-not $record.facts_verified -and -not $record.callback_verified -and -not $record.candidate_generation_verified) "$case no facts promotion"
        if($case -in @('good','empty-image')){
            Check (-not $failed -and $record.gui_completion_verified -and $record.heap_capture_succeeded -and $captures -eq 1) "$case one capture"
            Check ($record.paired_image_candidate_verified -eq ($case -ceq 'good')) "$case pair availability"
        }else{
            Check ($failed -and -not $record.heap_capture_succeeded -and -not $record.paired_image_candidate_verified) "$case no pair"
            Check ($record.gui_completion_verified -eq ($case -cne 'initial-generation-loss')) "$case preserved GUI evidence"
            Check ($captures -eq $(if($case -in @('initial-generation-loss','qt-copy-timeout','qt-postcopy-generation-loss')){0}else{1})) "$case no retry"
            if($case -ceq 'heap-hash-mismatch'){Check (-not $record.heap_saved_copy_verified) 'hash mismatch no saved-copy promotion'}
            if($case -in @('heap-timeout','heap-disconnect','heap-malformed')){Check $record.heap_capture_transport_unknown "$case remote uncertain"}
            if($case -in @('qt-copy-timeout','qt-postcopy-generation-loss')){Check ($record.qt_saved_copy_verified -eq ($case -ceq 'qt-postcopy-generation-loss')) "$case separate saved copy"}
        }
    }
    $loop=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.WhileStatementAst]},$true))[0]
    $statusLoop=[scriptblock]::Create('$statusTimeoutUsed=$false;'+$loop.Extent.Text)
    foreach($statusCase in @('one-timeout','two-timeouts','disconnect','exception')){
        $polls=0;$observationClock.ElapsedMilliseconds=100
        $record=[ordered]@{gui_completion_verified=$false;qt_image_available=$false;qt_saved_copy_verified=$false;heap_saved_copy_verified=$false;heap_capture_succeeded=$false;facts_verified=$false}
        function ObservationSSH([string]$command){
            $script:polls++
            if($statusCase -ceq 'exception'){throw 'fixture exception'}
            if($statusCase -ceq 'disconnect'){return @{timeout=$false;exit=255;stdout='partial'}}
            if($polls -eq 1 -or $statusCase -ceq 'two-timeouts'){return @{timeout=$true;exit=1;stdout='partial completion'}}
            $observationClock.ElapsedMilliseconds=150000;return @{timeout=$false;exit=3;stdout=''}
        }
        $failed=$false;try{. $statusLoop}catch{$failed=$true}
        Check ($failed -eq ($statusCase -cne 'one-timeout')) "$statusCase status allowance"
        Check (-not $record.gui_completion_verified -and -not $record.qt_image_available -and -not $record.heap_capture_succeeded -and -not $record.facts_verified) "$statusCase partial status grants nothing"
        Check ($polls -eq $(if($statusCase -in @('one-timeout','two-timeouts')){2}else{1})) "$statusCase finite polling"
    }
    # Execute original transport wrappers with a mock, proving remaining-clock caps.
    foreach($name in @('ObservationSSH','ObservationCopy')){. ([scriptblock]::Create(($functions|Where-Object Name -ceq $name).Extent.Text))}
    $budget=Get-FactsDevelopmentBudget
    function Native([string]$program,[string[]]$arguments,[int]$timeoutMs){$script:lastTimeout=$timeoutMs;return @{exit=0;timeout=$false;stdout=''}}
    $observationClock.ElapsedMilliseconds=149900
    [void](ObservationSSH 'fixture');Check ($lastTimeout -eq 100) 'SSH original remaining budget'
    [void](ObservationCopy '/fixture' 'fixture');Check ($lastTimeout -eq 100) 'SCP original remaining budget'
    $observationClock.ElapsedMilliseconds=150000
    $failed=$false;try{ObservationSSH 'fixture'}catch{$failed=$true};Check $failed 'no new live budget'
    Check ($source.IndexOf('/home/root/rem9-validation/screenshot ''$heapRemote''') -lt $source.IndexOf('}finally{')) 'heap before restoration'
    Check (-not $source.Contains('--image-only')) 'old helper exact interface'
    Check ($source.Contains("throw 'SOURCE ONLY:")) 'source execution guard'
    # Execute the specialized existing finally block with transport stubs.
    $outerTry=@($ast.EndBlock.Statements|Where-Object {$_ -is [Management.Automation.Language.TryStatementAst]})[0]
    $finallyText=$outerTry.Finally.Extent.Text
    $recovery=[scriptblock]::Create($finallyText.Substring(1,$finallyText.Length-2))
    . ([scriptblock]::Create(($functions|Where-Object Name -ceq 'Expand').Extent.Text))
    . ([scriptblock]::Create(($functions|Where-Object Name -ceq 'PreserveObservationImages').Extent.Text))
    $stock=[pscustomobject]@{stock_pid=8888;stock_start='9999'};$rollback='fixture-rollback';$fixtureCheck='fixture-only'
    $payloadHash='a'*64;$record=@{heap_capture_transport_unknown=$false}
    $arming=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.StringConstantExpressionAst] -and $node.Value.Contains('BEGIN private initial lock')},$true))[0].Value
    $expandedArming=Expand $arming
    $expectedArmingLine='test "$(sha256sum ''@ROOT@/payload.so'' | awk ''{print $1}'')" = ''@PAYLOADHASH@'''
    $expectedArmingLine=$expectedArmingLine.Replace('@ROOT@',$remote).Replace('@PAYLOADHASH@',$payloadHash)
    Check (($expandedArming.Replace("`r`n","`n").Split("`n")) -ccontains $expectedArmingLine) 'exact selected arming payload hash'
    Check (-not $expandedArming.Contains('0'*64) -and -not $expandedArming.Contains('@PAYLOADHASH@')) 'no unresolved arming hash'
    function SSH([string]$command){
        if($command.Contains('for name in payload.so')){$script:cleanupCommand=$command}
        if($command.Contains("printf 'absent\n'; else")){
            if($preservationCase -ceq 'preservation-transport-failure'){return @{exit=255;timeout=$false;stdout=''}}
            $qt=$command.Contains('/input-window.png')
            $present=if($qt){$preservationCase -cne 'absent'}else{$preservationCase -in @('heap-copy-failure','saved-both')}
            return @{exit=0;timeout=$false;stdout=$(if($present){"present $($png.Length) $pngHash`n"}else{"absent`n"})}
        }
        return @{exit=0;timeout=$false;stdout=$(if($command.Contains("printf 'present")){"absent`n"}else{''})}
    }
    foreach($preservationCase in @('absent','missed-completion','qt-copy-failure','qt-postcopy-guard-failure','heap-copy-failure','remote-unknown','saved-both','preservation-transport-failure')){
        $unknown=$preservationCase -ceq 'remote-unknown'
        $qtSaved=$preservationCase -in @('qt-postcopy-guard-failure','heap-copy-failure','remote-unknown','saved-both')
        $heapSaved=$preservationCase -ceq 'saved-both'
        $cleanupExpected=$preservationCase -in @('absent','qt-postcopy-guard-failure','saved-both')
        $cleanupCommand=''
        $record=[ordered]@{arm_intent=$true;restored=$false;cleanup_verified=$false;gui_completion_verified=($preservationCase -cne 'missed-completion');qt_image_available=$false;
            qt_saved_copy_verified=$qtSaved;qt_saved_copy_sha256=$pngHash;heap_saved_copy_verified=$heapSaved;heap_saved_copy_sha256=$pngHash;
            heap_capture_succeeded=$false;heap_capture_transport_unknown=$unknown;paired_image_candidate_verified=$false;
            facts_verified=$false;callback_verified=$false;candidate_generation_verified=$false}
        [IO.File]::WriteAllBytes((Join-Path $packet 'input-window.png'),$png)
        [IO.File]::WriteAllBytes((Join-Path $packet 'heap-after-gui.png'),$png)
        $failed=$false;try{. $recovery}catch{$failed=$true}
        Check $record.restored "finally restores $preservationCase"
        Check ($record.cleanup_verified -eq $cleanupExpected) "finally cleanup certainty $preservationCase"
        Check ($failed -eq (-not $cleanupExpected)) "finally preserves unresolved cleanup $preservationCase"
        Check ($record.gui_completion_verified -eq ($preservationCase -cne 'missed-completion') -and -not $record.qt_image_available -and -not $record.heap_capture_succeeded -and -not $record.paired_image_candidate_verified) "finally preserves live fields $preservationCase"
        Check (-not $record.facts_verified -and -not $record.callback_verified -and -not $record.candidate_generation_verified) "historical evidence grants no facts $preservationCase"
        Check ($cleanupCommand.Contains("rm -f '/tmp/rem25-facts-$nonce-input-observation.png'") -eq $cleanupExpected) "conditional deletion $preservationCase"
    }
    Write-Output "input-observation collector: PASS $count assertions (mock transport; no device)"
}finally{
    $resolved=[IO.Path]::GetFullPath($packet);$tempRoot=[IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if(-not $resolved.StartsWith($tempRoot,[StringComparison]::OrdinalIgnoreCase)){throw 'Test cleanup path escaped temp root'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
