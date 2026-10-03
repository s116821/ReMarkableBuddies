param([Parameter(Mandatory=$true)][string]$PayloadPath,
      [Parameter(Mandatory=$true)][string]$EvidenceDirectory, [switch]$PrepareOnly)
$ErrorActionPreference = 'Stop'
$nonce='719b74a2c4e54b3bbdbf9e389f03c2b1'
$remote='/run/rmb-qt-probe-'+$nonce
$rollback='rmb-qt-probe-'+$nonce+'-rollback'
$payloadHash='572647c391dd949a2f6b4dcafa5ff6b0e3d6a7fa3f17d69405776f8efdc987a6'
if((Get-FileHash -LiteralPath $PayloadPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $payloadHash){throw 'Payload changed'}
$packet=Join-Path $EvidenceDirectory ('qt-startup-packet-'+$nonce)
if(-not(Test-Path -LiteralPath $packet)){[void](New-Item -ItemType Directory -Path $packet)}
$files=@{}
foreach($name in @('launch.sh','restore.sh','native-probe.conf')){
    $text=[IO.File]::ReadAllText((Join-Path $PSScriptRoot $name)).Replace("`r`n","`n")
    $path=Join-Path $packet $name
    if(Test-Path -LiteralPath $path){if([IO.File]::ReadAllText($path) -cne $text){throw 'Prepared file changed'}}
    else{[IO.File]::WriteAllText($path,$text,[Text.UTF8Encoding]::new($false))}
    $files[$name]=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
}
foreach($item in @{owner=$nonce;'dropin.sha256'=$files['native-probe.conf']}.GetEnumerator()){
    $path=Join-Path $packet $item.Key
    if(Test-Path -LiteralPath $path){if([IO.File]::ReadAllText($path) -cne $item.Value){throw 'Prepared token changed'}}
    else{[IO.File]::WriteAllText($path,$item.Value,[Text.UTF8Encoding]::new($false))}
    $files[$item.Key]=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
}
if($PrepareOnly){$files|ConvertTo-Json;return}
$record=[ordered]@{nonce=$nonce;experiment='explicit-fixture-native-page-creation';payload_source='bed8ac086b806272e08f33075e4835b86d70c514';payload_sha256=$payloadHash;operator_sha256=(Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant();files=$files;results=@();arm_intent=$false;armed=$false;callback_verified=$false;candidate_generation_verified=$false;qml_access_verified=$false;candidate_receipt=$null;restored=$false;cleanup_verified=$false;diagnostic_collected=$false;diagnostic_sha256=$null;final_callback_state='not-checked';final_callback_collected=$false;final_callback_sha256=$null}
function Native([string]$program,[string[]]$arguments,[int]$timeoutMs=20000){
    $info=[Diagnostics.ProcessStartInfo]::new();$info.FileName=$program;$info.UseShellExecute=$false
    $info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
    foreach($argument in $arguments){$info.ArgumentList.Add($argument)}
    $process=[Diagnostics.Process]::Start($info)
    $stdout=$process.StandardOutput.ReadToEndAsync();$stderr=$process.StandardError.ReadToEndAsync()
    $timeout=-not $process.WaitForExit($timeoutMs)
    if($timeout){$process.Kill($true);if(-not $process.WaitForExit(3000)){throw 'Local transport exit uncertain'}}
    $result=[ordered]@{program=$program;exit=$process.ExitCode;timeout=$timeout;stdout=$stdout.GetAwaiter().GetResult();stderr=$stderr.GetAwaiter().GetResult()}
    $process.Dispose();$record.results+=$result;return $result
}
function SSH([string]$command){Native 'ssh' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8','RM2',$command)}
function ObservationSSH([string]$command){
    $remaining=35000-$observationClock.ElapsedMilliseconds
    if($remaining -le 0){throw 'Callback observation budget exhausted; restoration still required'}
    $result=Native 'ssh' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=2','RM2',$command) ([int][Math]::Min(3000,$remaining))
    if($observationClock.ElapsedMilliseconds -ge 35000){throw 'Callback observation returned outside budget; restoration still required'}
    return $result
}
function Require($result){if($result.timeout -or $result.exit -ne 0){throw 'One-shot stage failed; keep evidence and rollback duty'}}
function Expand([string]$text){$text.Replace('@ROOT@',$remote).Replace('@NONCE@',$nonce).Replace('@UNIT@',$rollback)}
try{
    Require (SSH (Expand @'
set -eu
test "$(sed -n 's/^IMG_VERSION=//p' /etc/os-release)" = '"3.28.0.172"'
test "$(cat /sys/devices/soc0/machine)" = 'reMarkable 2.0'
test "$(pidof xochitl)" = 4349
test "$(awk '{print $22}' /proc/4349/stat)" = 190863366
test "$(sha256sum /proc/4349/exe | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
test "$(sha256sum /usr/lib/libQt6Core.so.6.10.3 | awk '{print $1}')" = 43b0e210d64e59b534490d78c4c82cc1d2958999b0969aa0f77e11a082704e5d
test "$(sha256sum /usr/lib/libQt6Qml.so.6.10.3 | awk '{print $1}')" = e9cfb062609972005470d048f75b68c749f0ccd3bf09de45916232845694c944
test "$(sha256sum /usr/lib/libQt6Gui.so.6.10.3 | awk '{print $1}')" = 93fe582cc61673342ca49e12306d7f689860016582fa46ce135ffa280a972839
test "$(sha256sum /usr/lib/libQt6Quick.so.6.10.3 | awk '{print $1}')" = b20f9adaefe8cebeca445906e891370f75f0b0af878ac1b35e0c91159778628f
test "$(sha256sum /usr/lib/systemd/system/xochitl.service | awk '{print $1}')" = adb0a2654ce9ec884f67c0627c22d539d6475dd0af80816819ee80bf13c0e6d6
test "$(sha256sum /usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf | awk '{print $1}')" = b15560e1dca2f4451b59537c490015aa2f5ea691437bd7ddddc7a65411aaad6e
test "$(systemctl show --property=RestartMode --value xochitl.service)" = direct
test "$(systemctl show --property=Restart --value xochitl.service)" = on-failure
test "$(systemctl show --property=KillMode --value xochitl.service)" = control-group
test "$(systemctl show --property=NRestarts --value xochitl.service)" = 0
test -z "$(systemctl show --property=Job --value xochitl.service)"
if awk 'BEGIN {RS="\0"} /^(LD_PRELOAD|LD_LIBRARY_PATH|XOVI_ROOT)=/ {found=1} END {exit !found}' /proc/4349/environ; then exit 90; fi
systemctl is-active xochitl.service reader-buddy.service rm-sync.service
test ! -e '@ROOT@'
test ! -e /run/systemd/system/xochitl.service.d
test ! -L /run/systemd/system/xochitl.service.d
test ! -e '/run/systemd/transient/@UNIT@.service'
test ! -e '/run/systemd/transient/@UNIT@.timer'
mkdir -m700 '@ROOT@'
'@))
    foreach($name in $files.Keys){Require (Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',(Join-Path $packet $name),('RM2:'+$remote+'/'+$name)))}
    Require (Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',$PayloadPath,('RM2:'+$remote+'/payload.so')))
    foreach($name in $files.Keys){Require (SSH ("set -eu; test `"`$(sha256sum '$remote/$name' | awk '{print `$1}')`" = '$($files[$name])'; chmod 600 '$remote/$name'"))}
    # Observation admission budget starts before arming. Recovery and final
    # evidence collection keep their separate mandatory duty after this cutoff.
    $observationClock=[Diagnostics.Stopwatch]::StartNew()
    $record.arm_intent=$true
    Require (SSH (Expand @'
set -eu
test "$(sha256sum '@ROOT@/payload.so' | awk '{print $1}')" = 572647c391dd949a2f6b4dcafa5ff6b0e3d6a7fa3f17d69405776f8efdc987a6
chmod 600 '@ROOT@/payload.so'
# Prove actual target flags before arming or stopping any original service.
exec 9>'@ROOT@/admission.lock'
flock -n 9
flock -u 9
exec 9>&-
systemd-run --unit='@UNIT@' --on-active=45s --timer-property=AccuracySec=1s --property=Type=oneshot --property=RemainAfterExit=yes --property=Restart=no --property=TimeoutStartSec=240s --property=TimeoutStopSec=5s --property=KillMode=control-group --property=UMask=0077 /bin/sh '@ROOT@/restore.sh'
test "$(systemctl show --property=ActiveState --value '@UNIT@.timer')" = active
test "$(systemctl show --property=OnFailure --value '@UNIT@.service')" = ''
test "$(systemctl show --property=FailureAction --value '@UNIT@.service')" = none
'@))
    $record.armed=$true
    Require (SSH (Expand @'
set -eu
systemctl stop reader-buddy.service
exec 9>'@ROOT@/admission.lock'
flock -n 9
test ! -e '@ROOT@/entry.closed'
test ! -e '@ROOT@/restore.claim'
test ! -e /run/systemd/system/xochitl.service.d
mkdir -m755 /run/systemd/system/xochitl.service.d
stat -c '%d %i %a %u' /run/systemd/system/xochitl.service.d > '@ROOT@/parent.signature'
cp '@ROOT@/native-probe.conf' '/run/systemd/system/xochitl.service.d/zz-rmb-qt-probe-@NONCE@.conf'
chmod 644 '/run/systemd/system/xochitl.service.d/zz-rmb-qt-probe-@NONCE@.conf'
systemctl daemon-reload
case "$(systemctl show --property=ExecStart --value xochitl.service)" in *'path=/bin/sh ; argv[]=/bin/sh @ROOT@/launch.sh ;'*) :;; *) exit 90;; esac
test "$(systemctl show --property=Restart --value xochitl.service)" = on-failure
test "$(systemctl show --property=RestartMode --value xochitl.service)" = direct
flock -u 9
exec 9>&-
systemctl restart xochitl.service
'@))
    for($attempt=0;$attempt -lt 35;$attempt++){
        $observed=ObservationSSH (Expand "test -f '@ROOT@/callback.json' && test `"`$(wc -c < '@ROOT@/callback.json')`" -le 256 && cat '@ROOT@/callback.json'")
        if(-not $observed.timeout -and $observed.exit -eq 0){
            [IO.File]::WriteAllText((Join-Path $packet 'callback.json'),$observed.stdout,[Text.UTF8Encoding]::new($false))
            $callback=$observed.stdout|ConvertFrom-Json
            $record.callback_verified=$callback.nonce -is [string] -and $callback.nonce -ceq $nonce -and $callback.application_thread -is [bool] -and $callback.application_thread
            $record.candidate_receipt=$callback
            $qmlFieldsMatch=$record.callback_verified -and $callback.stage -is [string] -and $callback.stage -ceq 'resolved' -and $callback.engine_thread -is [bool] -and $callback.engine_thread -and $callback.helper_available -is [bool] -and $callback.helper_available -and $callback.controller_available -is [bool] -and $callback.controller_available
            Require (ObservationSSH (Expand @'
set -eu
read p started < '@ROOT@/attempt.identity'
case "$p:$started" in *[!0-9:]*|:*|*:) exit 90;; esac
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(sha256sum "/proc/$p/exe" | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test -z "$(systemctl show --property=Job --value xochitl.service)"
printf 'new-generation-executable-verified\n'
'@))
            $record.candidate_generation_verified=$true
            $record.qml_access_verified=$qmlFieldsMatch
            break
        }
        if($observed.timeout){throw 'Callback observation transport unknown'}
        $remaining=35000-$observationClock.ElapsedMilliseconds
        if($remaining -le 0){throw 'Callback observation budget exhausted; restoration still required'}
        Start-Sleep -Milliseconds ([int][Math]::Min(1000,$remaining))
    }
}finally{
    try{
    if($record.arm_intent){
        # Same singleton service; never execute the restore script in a second actor.
        $trigger=SSH (Expand "systemctl start --no-block '@UNIT@.service'")
        for($check=0;$check -lt 30;$check++){
            $restoration=SSH (Expand "test -f '@ROOT@/restored' && test `"`$(cat '@ROOT@/restored')`" = '@NONCE@ stock-verified' && test `"`$(systemctl show --property=ActiveState --value '@UNIT@.service')`" = active && test `"`$(systemctl show --property=SubState --value '@UNIT@.service')`" = exited && test `"`$(systemctl show --property=ExecMainStatus --value '@UNIT@.service')`" = 0 && /bin/sh '@ROOT@/restore.sh' --verify && systemctl is-active xochitl.service reader-buddy.service rm-sync.service && systemctl show --property=MainPID --property=Job xochitl.service")
            if(-not $restoration.timeout -and $restoration.exit -eq 0){$record.restored=$true;break}
            if($restoration.timeout){break}
            Start-Sleep -Seconds 1
        }
        if($record.restored){
            # Final evidence only: never retrofit the live callback/generation
            # qualification flags after the attempted process has been restored.
            $finalCallbackState=SSH (Expand @'
set -eu
if test -f '@ROOT@/callback.json'; then
    test ! -L '@ROOT@/callback.json'
    test "$(stat -c '%a %u' '@ROOT@/callback.json')" = '600 0'
    test "$(wc -c < '@ROOT@/callback.json')" -le 256
    printf 'present\n'
else
    test ! -e '@ROOT@/callback.json'
    test ! -L '@ROOT@/callback.json'
    printf 'absent\n'
fi
'@)
            Require $finalCallbackState
            $record.final_callback_state=$finalCallbackState.stdout.Trim()
            if($record.final_callback_state -ceq 'present'){
                $finalCallbackPath=Join-Path $packet 'callback-final.json'
                Require (Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',('RM2:'+$remote+'/callback.json'),$finalCallbackPath))
                $record.final_callback_sha256=(Get-FileHash -LiteralPath $finalCallbackPath -Algorithm SHA256).Hash.ToLowerInvariant()
                $record.final_callback_collected=$true
            }elseif($record.final_callback_state -cne 'absent'){throw 'Final callback presence uncertain; retain exact stage'}
            $diagnosticState=SSH (Expand @'
set -eu
if test -f '@ROOT@/diagnostics.json'; then
    test ! -L '@ROOT@/diagnostics.json'
    test "$(stat -c '%a %u' '@ROOT@/diagnostics.json')" = '600 0'
    test "$(wc -c < '@ROOT@/diagnostics.json')" -le 8192
    printf 'present\n'
else
    test ! -e '@ROOT@/diagnostics.json'
    test ! -L '@ROOT@/diagnostics.json'
    printf 'absent\n'
fi
'@)
            Require $diagnosticState
            if($diagnosticState.stdout.Trim() -ceq 'present'){
                $diagnosticPath=Join-Path $packet 'diagnostics.json'
                Require (Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',('RM2:'+$remote+'/diagnostics.json'),$diagnosticPath))
                $record.diagnostic_sha256=(Get-FileHash -LiteralPath $diagnosticPath -Algorithm SHA256).Hash.ToLowerInvariant()
                $record.diagnostic_collected=$true
            }elseif($diagnosticState.stdout.Trim() -cne 'absent'){throw 'Diagnostic presence uncertain; retain exact stage'}
            foreach($name in @('attempt.claim','attempt.identity','restore.claim','restored','parent.signature','entry.closed')){
                $collected=Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',('RM2:'+$remote+'/'+$name),(Join-Path $packet $name))
                Require $collected
            }
            foreach($name in @('restore.failure','verification.first-refusal')){
                $diagnostic=SSH ("if test -f '$remote/$name'; then test `"`$(wc -c < '$remote/$name')`" -le 128 && cat '$remote/$name'; fi")
                Require $diagnostic
                if($diagnostic.stdout){[IO.File]::WriteAllText((Join-Path $packet $name),$diagnostic.stdout,[Text.UTF8Encoding]::new($false))}
            }
            $cleaned=SSH (Expand @'
set -eu
/bin/sh '@ROOT@/restore.sh' --verify
systemctl stop '@UNIT@.timer' '@UNIT@.service'
for wait in 1 2 3 4 5; do
    if test ! -e '/run/systemd/transient/@UNIT@.timer' && test ! -e '/run/systemd/transient/@UNIT@.service'; then break; fi
    sleep 1
done
test ! -e '/run/systemd/transient/@UNIT@.timer'
test ! -e '/run/systemd/transient/@UNIT@.service'
test ! -e '/run/systemd/system/xochitl.service.d/zz-rmb-qt-probe-@NONCE@.conf'
for name in payload.so launch.sh restore.sh native-probe.conf owner dropin.sha256 callback.json attempt.claim attempt.identity restore.claim restored parent.signature admission.lock entry.closed restore.failure verification.first-refusal diagnostics.json; do rm -f '@ROOT@/'"$name"; done
rmdir '@ROOT@'
test ! -e '@ROOT@'
systemctl is-active xochitl.service reader-buddy.service rm-sync.service
printf 'stock-restored-and-exact-stage-removed\n'
'@)
            $record.cleanup_verified=-not $cleaned.timeout -and $cleaned.exit -eq 0
        }
    }
    }finally{
    [IO.File]::WriteAllText((Join-Path $packet 'operator-receipt.json'),($record|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
    Write-Output ('receipt='+ (Join-Path $packet 'operator-receipt.json'))
    Write-Output ('restored='+$record.restored+' cleanup_verified='+$record.cleanup_verified)
    }
    if(-not $record.cleanup_verified){throw 'Restoration/stage uncertain; preserve exact path and timer duty, no retry'}
    if(-not $record.callback_verified){throw 'Stock restored; callback nonce/thread proof missing or failed'}
    if(-not $record.qml_access_verified){throw 'Stock restored; existing-engine QML availability proof refused or incomplete'}
}
