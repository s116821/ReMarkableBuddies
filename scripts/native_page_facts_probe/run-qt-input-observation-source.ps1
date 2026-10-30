param([Parameter(Mandatory=$true)][string]$PayloadPath,
      [Parameter(Mandatory=$true)][string]$PublisherPath,
      [Parameter(Mandatory=$true)][string]$ExpectedPath,
      [Parameter(Mandatory=$true)][string]$StockBaselinePath,
      [Parameter(Mandatory=$true)][string]$EvidenceDirectory, [switch]$PrepareOnly)
$ErrorActionPreference='Stop'
throw 'SOURCE ONLY: requires a separately selected fresh nonce, artifacts, baseline and exact packet review; spent literals below are reference placeholders.'
$developmentEvdevLogging=$false # Fixed source opt-in, no runtime parameter.
$evdevBegin=$null;$evdevEnd=$null;$evdevEndpointReason='missing-endpoints'
$nonce='b3e3ca0475a84432a9b328218395de0c'
$remote='/run/rmb-qt-probe-'+$nonce
$rollback='rmb-qt-probe-'+$nonce+'-rollback'
$payloadHash='0000000000000000000000000000000000000000000000000000000000000000'
$publisherHash='0000000000000000000000000000000000000000000000000000000000000000'
$expectedHash='38f71a7ae29f4c652559500e3adaca5a3498104261aef913562f9f6110a02c51'
$stockBaselineHash='ff23a36b74b18c1a6c82d9ad8321911d0e6f6bbecc71e9aabcffdddc4fdad30d'
function Hash([string]$path){(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}
$proofPath=Join-Path $PSScriptRoot 'facts-proof.ps1'
$budgetPath=Join-Path $PSScriptRoot 'development-budget.ps1'
if((Hash $proofPath) -cne 'ea0736d4025ef1d929e888a5755a59c506a580e067bdb8a8bb4134b6e3cf6f73' -or
   (Hash $budgetPath) -cne 'a2956bc0f5073dc6eb69c2b87c5f7c99e2bdb8828c3ae410e73a295c8f0b4ba6'){throw 'Fixed proof/budget source changed'}
$refusalProofPath=Join-Path $PSScriptRoot 'facts-refusal-proof.ps1'
if((Hash $refusalProofPath) -cne '44c786488a88fb249501bbcc0361fe53b561f9b5ac33e8f3ae1ea22b37732c66'){throw 'Fixed refusal decoder changed'}
. $refusalProofPath
. $proofPath
$observationProofPath=Join-Path $PSScriptRoot 'input-observation-proof.ps1'
if((Hash $observationProofPath) -cne 'b1a7338fb816fc9891d5247c1f4e0c0c8d0ff94d08fb4dc98065b0357b455917'){throw 'Fixed observation decoder changed'}
. $observationProofPath
$evdevProofPath=Join-Path $PSScriptRoot 'input-evdev-journal.ps1'
if((Hash $evdevProofPath) -cne 'e330756724b9ae978f41f3e375884175281fbc9305cdea8a203eff18e3b09a7c'){throw 'Fixed evdev journal source changed'}
. $evdevProofPath
. $budgetPath
$budget=Get-FactsDevelopmentBudget
$budget.SetupSelection='main-dev-input-observation-120s'
$budget.ReadClockOrigin='accepted-observation-end-monotonic'
if((Hash $PayloadPath) -cne $payloadHash -or (Hash $PublisherPath) -cne $publisherHash -or (Hash $ExpectedPath) -cne $expectedHash -or (Hash $StockBaselinePath) -cne $stockBaselineHash){throw 'Frozen candidate input changed'}
$expected=Get-Content -LiteralPath $ExpectedPath -Raw|ConvertFrom-Json
$stock=Get-Content -LiteralPath $StockBaselinePath -Raw|ConvertFrom-Json
if($expected.nonce -cne $nonce -or $stock.nonce -isnot [string] -or $stock.nonce -cne $nonce -or
   ($stock.stock_pid -isnot [long] -and $stock.stock_pid -isnot [int])){throw 'Stock baseline nonce/PID type refused'}
if($stock.stock_pid -le 1 -or $stock.stock_start -isnot [string] -or $stock.stock_start -cnotmatch '^[1-9][0-9]*$' -or
   $stock.content_sha256 -isnot [string] -or $stock.content_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
   $stock.fixture_evidence_sha256 -isnot [string] -or $stock.fixture_evidence_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
   $stock.document -isnot [string] -or $stock.document -cne $expected.document -or $stock.order -isnot [array] -or
   ($stock.order -join ',') -cne ($expected.order -join ',')){throw 'Fresh Main fixture/generation baseline refused'}
$fixtureCommands=@("cd '/home/root/.local/share/remarkable/xochitl'")
if($stock.fixture_manifest -isnot [array] -or $stock.fixture_manifest.Count -ne 14 -or
   @($stock.fixture_manifest.path|Select-Object -Unique).Count -ne 14){throw 'Fixed fixture manifest refused'}
foreach($entry in $stock.fixture_manifest){
    if($entry.path -isnot [string] -or $entry.path -cnotmatch '^[0-9a-f-]{36}(\.[a-z]+|/[0-9a-f-]{36}\.rm|\.thumbnails/[0-9a-f-]{36}\.png)$' -or
       -not $entry.path.StartsWith($expected.document,[StringComparison]::Ordinal) -or
       $entry.sha256 -isnot [string] -or $entry.sha256 -cnotmatch '^[0-9a-f]{64}$'){throw 'Fixed fixture path/hash refused'}
    $fixtureCommands+=('test -f ''{0}'' && test ! -L ''{0}''; test "$(sha256sum ''{0}'' | awk ''{{print $1}}'')" = ''{1}''' -f $entry.path,$entry.sha256)
}
$fixtureCheck=$fixtureCommands -join "`n"
$packet=Join-Path $EvidenceDirectory ('qt-startup-packet-'+$nonce)
if(-not(Test-Path -LiteralPath $packet)){[void](New-Item -ItemType Directory -Path $packet)}
function Freeze([string]$name,[string]$text){
    $path=Join-Path $packet $name
    if(Test-Path -LiteralPath $path){if([IO.File]::ReadAllText($path) -cne $text){throw "Prepared file changed: $name"}}
    else{[IO.File]::WriteAllText($path,$text,[Text.UTF8Encoding]::new($false))}
    return Hash $path
}
$files=[ordered]@{}
foreach($name in @('launch.sh','restore.sh','native-probe.conf','check-input-observation-ready.sh','publish-input-observation-end-once.sh')){
    $fileText=[IO.File]::ReadAllText((Join-Path $PSScriptRoot $name)).Replace("`r`n","`n")
    if($name -ceq 'launch.sh' -and $developmentEvdevLogging){$fileText=$fileText.Replace('development_evdev_logging=0','development_evdev_logging=1')}
    $files[$name]=Freeze $name $fileText
}
$files.owner=Freeze 'owner' $nonce
$files['dropin.sha256']=Freeze 'dropin.sha256' $files['native-probe.conf']
$localBindings=[ordered]@{nonce=$nonce;operator_sha256=(Hash $PSCommandPath);stock_baseline_sha256=(Hash $StockBaselinePath);
 expected_sha256=$expectedHash;payload_sha256=$payloadHash;publisher_sha256=$publisherHash;proof_sha256=(Hash $proofPath);budget_sha256=(Hash $budgetPath);observation_proof_sha256=(Hash $observationProofPath);evidence_profile='device-frames-v1';heap_image_profile='legacy-df745-overview-768x1024';development_evdev_logging=$developmentEvdevLogging;evdev_journal_source_sha256=(Hash $evdevProofPath);capture_helper_sha256='df745d56a2ef1834ea644b8859646972210c5e0247a3e6e75e9e245f5fa919b8';files=$files}
[void](Freeze 'packet-bindings.json' ($localBindings|ConvertTo-Json -Depth 5))
if($PrepareOnly){$localBindings|ConvertTo-Json -Depth 5;return}
# Main alone executes after independent artifact/operator review and advance notice.
$record=[ordered]@{nonce=$nonce;experiment='development-input-observation';evidence_profile='device-frames-v1';heap_image_profile='legacy-df745-overview-768x1024';payload_source='unselected-source-only';publisher_source='unselected-source-only';bindings=$localBindings;budget=$budget;results=@();arm_intent=$false;armed=$false;callback_verified=$false;candidate_generation_verified=$false;facts_verified=$false;observation_generation_verified=$false;gui_completion_verified=$false;qt_image_available=$false;qt_saved_copy_verified=$false;qt_saved_copy_sha256=$null;heap_saved_copy_verified=$false;heap_saved_copy_sha256=$null;heap_capture_succeeded=$false;heap_capture_transport_unknown=$false;paired_image_candidate_verified=$false;heap_capture_start_ms=$null;heap_capture_end_ms=$null;candidate_receipt=$null;restored=$false;cleanup_verified=$false;live_diagnostics_collected=$false;live_diagnostics_sha256=$null;gate_evidence=@{};diagnostic_collected=$false;diagnostic_sha256=$null;final_callback_state='not-checked';final_callback_collected=$false;final_callback_sha256=$null;live_observation_ms=$null;live_refusal=$null;live_refusal_callback_match=$false;final_refusal=$null;final_refusal_callback_match=$false}
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
function SSH([string]$command){Native 'ssh' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8','RM2',$command.Replace("`r`n","`n"))}
function ObservationSSH([string]$command){
    $remaining=$budget.LiveObservationMs-$observationClock.ElapsedMilliseconds
    if($remaining -le 0){throw 'Callback observation budget exhausted; restoration still required'}
    $result=Native 'ssh' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=2','RM2',$command.Replace("`r`n","`n")) ([int][Math]::Min(3000,$remaining))
    if($observationClock.ElapsedMilliseconds -ge $budget.LiveObservationMs){throw 'Callback observation returned outside budget; restoration still required'}
    return $result
}
function Require($result){if($result.timeout -or $result.exit -ne 0){throw 'One-shot stage failed; keep evidence and rollback duty'}}
function ObservationCopy([string]$remotePath,[string]$localPath){
    $remaining=$budget.LiveObservationMs-$observationClock.ElapsedMilliseconds
    if($remaining -le 0){throw 'Image collection budget exhausted; restoration required'}
    $result=Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=2',('RM2:'+$remotePath),$localPath) ([int][Math]::Min(3000,$remaining))
    if($observationClock.ElapsedMilliseconds -ge $budget.LiveObservationMs){throw 'Image collection outside original clock; restoration required'}
    return $result
}
function RequireSameObservationIdentity {
    $checked=ObservationSSH $identityCommand
    Require $checked
    if($checked.stdout -cne $identity.stdout){throw 'Same candidate generation lost; restoration required'}
}
function PreserveObservationImages {
    # Fixed post-restoration deletion gate, with no image collection or live upgrade.
    $preserved=$true;$record.image_preservation_errors=@()
    foreach($image in @(
        @{remote=($remote+'/input-window.png');local='input-window.png';verified=$record.qt_saved_copy_verified;sha=$record.qt_saved_copy_sha256},
        @{remote=('/tmp/rem25-facts-'+$nonce+'-input-observation.png');local='heap-after-gui.png';verified=$record.heap_saved_copy_verified;sha=$record.heap_saved_copy_sha256})){
        if($image.local -ceq 'heap-after-gui.png' -and $record.heap_capture_transport_unknown){
            $preserved=$false;$record.image_preservation_errors+='heap capture may still be running; retain exact stage and external path';continue
        }
        try{
            $path=$image.remote
            $state=SSH ("set -eu; if test ! -e '$path' && test ! -L '$path'; then printf 'absent\n'; else test -f '$path'; test ! -L '$path'; bytes=`$(wc -c < '$path'); test `"`$bytes`" -le 8388608; hash=`$(sha256sum '$path' | awk '{print `$1}'); printf 'present %s %s\n' `"`$bytes`" `"`$hash`"; fi")
            Require $state
            if($state.stdout -ceq "absent`n"){continue}
            if($state.stdout.Length -gt 128 -or $state.stdout -cnotmatch '^present ([0-9]{1,7}) ([0-9a-f]{64})\n$'){throw 'Historical image metadata refused'}
            $bytes=[long]$Matches[1];$sha=$Matches[2];$local=Join-Path $packet $image.local
            if(-not $image.verified -or $image.sha -cne $sha -or -not(Test-Path -LiteralPath $local) -or
               (Get-Item -LiteralPath $local).Length -ne $bytes -or (Hash $local) -cne $sha){throw 'No verified saved image copy; retain exact stage/output path'}
        }catch{
            $preserved=$false;$record.image_preservation_errors+=($image.remote+': '+$_.Exception.Message)
        }
    }
    return $preserved
}
function Expand([string]$text){$evdevMarker=if($developmentEvdevLogging){"printf 'EVDEV_BEGIN_$nonce '; "+$InputEvdevSnapshotCommand}else{''};$text=$text.Replace('@EVDEVBEGIN@',$evdevMarker);$heapCleanup=if($record.heap_capture_transport_unknown){'# uncertain heap transport: preserve exact external path'}else{"rm -f '/tmp/rem25-facts-$nonce-input-observation.png'"};$text.Replace('@HEAPCLEANUP@',$heapCleanup).Replace('@PAYLOADHASH@',$payloadHash).Replace('@ROOT@',$remote).Replace('@NONCE@',$nonce).Replace('@UNIT@',$rollback).Replace('@STOCKPID@',[string]$stock.stock_pid).Replace('@STOCKSTART@',$stock.stock_start).Replace('@FIXTURECHECK@',$fixtureCheck)}
try{
    if($developmentEvdevLogging){
        Require (SSH (Expand @'
set -eu
env_records=$(set -o pipefail || exit 90; cat /proc/@STOCKPID@/environ | LC_ALL=C tr '\n' '\r' | LC_ALL=C tr '\000' '\n') || exit 90
printf '%s\n' "$env_records" | LC_ALL=C awk '/^(QT_QPA_EVDEV_DEBUG|QT_LOGGING_RULES|QT_MESSAGE_PATTERN)=/ {found=1} END {if(found) exit 90}'
'@))
    }
    Require (SSH (Expand @'
set -eu
test "$(sed -n 's/^IMG_VERSION=//p' /etc/os-release)" = '"3.28.0.172"'
test "$(cat /sys/devices/soc0/machine)" = 'reMarkable 2.0'
test "$(pidof xochitl)" = @STOCKPID@
test "$(awk '{print $22}' /proc/@STOCKPID@/stat)" = @STOCKSTART@
test "$(sha256sum /proc/@STOCKPID@/exe | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
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
env_records=$(set -o pipefail || exit 90; cat /proc/@STOCKPID@/environ | LC_ALL=C tr '\n' '\r' | LC_ALL=C tr '\000' '\n') || exit 90
printf '%s\n' "$env_records" | LC_ALL=C awk '/^(LD_PRELOAD|LD_LIBRARY_PATH|XOVI_ROOT)=/ {found=1} END {if(found) exit 90}'
systemctl is-active xochitl.service reader-buddy.service rm-sync.service
@FIXTURECHECK@
test ! -e '@ROOT@'
test ! -e /run/systemd/system/xochitl.service.d
test ! -L /run/systemd/system/xochitl.service.d
test ! -e '/run/systemd/transient/@UNIT@.service'
test ! -e '/run/systemd/transient/@UNIT@.timer'
mkdir -m700 '@ROOT@'
'@))
    foreach($name in $files.Keys){Require (Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',(Join-Path $packet $name),('RM2:'+$remote+'/'+$name)))}
    Require (Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',$PayloadPath,('RM2:'+$remote+'/payload.so')))
    Require (Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',$PublisherPath,('RM2:'+$remote+'/input-observation-end-publisher')))
    Require (SSH ("set -eu; test `"`$(sha256sum '$remote/input-observation-end-publisher' | awk '{print `$1}')`" = '$publisherHash'; chmod 700 '$remote/input-observation-end-publisher'"))
    foreach($name in $files.Keys){Require (SSH ("set -eu; test `"`$(sha256sum '$remote/$name' | awk '{print `$1}')`" = '$($files[$name])'; chmod 600 '$remote/$name'"))}
    # Observation admission budget starts before arming. Recovery and final
    # evidence collection keep their separate mandatory duty after this cutoff.
    $observationClock=[Diagnostics.Stopwatch]::StartNew()
    $record.arm_intent=$true
    Require (SSH (Expand @'
set -eu
test "$(sha256sum '@ROOT@/payload.so' | awk '{print $1}')" = '@PAYLOADHASH@'
chmod 600 '@ROOT@/payload.so'
# Prove actual target flags before arming or stopping any original service.
# BEGIN private initial lock (owned shell regression extracts this exact block).
umask 077
test ! -e '@ROOT@/admission.lock' && test ! -L '@ROOT@/admission.lock' || exit 90
(set -C; : > '@ROOT@/admission.lock')
test -f '@ROOT@/admission.lock' && test ! -L '@ROOT@/admission.lock' || exit 90
test "$(stat -c '%a %u' '@ROOT@/admission.lock')" = '600 0'
exec 9<>'@ROOT@/admission.lock'
flock -n 9
flock -u 9
exec 9>&-
# END private initial lock
systemd-run --unit='@UNIT@' --on-active=180s --timer-property=AccuracySec=1s --property=Type=oneshot --property=RemainAfterExit=yes --property=Restart=no --property=TimeoutStartSec=240s --property=TimeoutStopSec=5s --property=KillMode=control-group --property=UMask=0077 /bin/sh '@ROOT@/restore.sh'
test "$(systemctl show --property=ActiveState --value '@UNIT@.timer')" = active
test "$(systemctl show --property=OnFailure --value '@UNIT@.service')" = ''
test "$(systemctl show --property=FailureAction --value '@UNIT@.service')" = none
'@))
    $record.armed=$true
    $activation=SSH (Expand @'
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
@EVDEVBEGIN@
systemctl restart xochitl.service
'@)
    Require $activation
    if($developmentEvdevLogging){
        $matched=[regex]::Match($activation.stdout,'(?m)^EVDEV_BEGIN_'+$nonce+' ([^\r\n]*\n)')
        if($matched.Success){$evdevBegin=Get-InputEvdevEndpoint $matched.Groups[1].Value}
    }
    # Main performs positive observation readiness proof and the selected input interval,
    # and the fixed publisher once while this same host collector remains active.
    # Read-only observations are finite; no setup sleeps or physical retries.
    $statusTimeoutUsed=$false
    while(Test-FactsLiveObservationWindow $observationClock.ElapsedMilliseconds){
        $observed=ObservationSSH (Expand @'
set -eu
test -d '@ROOT@' && test ! -L '@ROOT@' || exit 90
test "$(stat -c '%a %u' '@ROOT@')" = '700 0' || exit 90
test "$(cat '@ROOT@/owner')" = '@NONCE@' || exit 90
test ! -e '@ROOT@/entry.closed' && test ! -L '@ROOT@/entry.closed' || exit 90
test ! -e '@ROOT@/restore.claim' && test ! -L '@ROOT@/restore.claim' || exit 90
if test ! -e '@ROOT@/input-observation-complete.json' && test ! -L '@ROOT@/input-observation-complete.json'; then exit 3; fi
test -f '@ROOT@/input-observation-complete.json' && test ! -L '@ROOT@/input-observation-complete.json' || exit 90
test "$(stat -c '%a %u' '@ROOT@/input-observation-complete.json')" = '600 0' || exit 90
test "$(wc -c < '@ROOT@/input-observation-complete.json')" -le 8192 || exit 90
cat '@ROOT@/input-observation-complete.json'
'@)
        if(-not $observed.timeout -and $observed.exit -eq 0){
            $identityCommand=Expand @'
set -eu
test ! -e '@ROOT@/entry.closed'
test ! -e '@ROOT@/restore.claim'
test -f '@ROOT@/attempt.identity' && test ! -L '@ROOT@/attempt.identity' || exit 90
test "$(stat -c '%a %u' '@ROOT@/attempt.identity')" = '600 0'
test "$(wc -c < '@ROOT@/attempt.identity')" -le 128
read p started < '@ROOT@/attempt.identity'
case "$p:$started" in *[!0-9:]*|:*|*:) exit 90;; esac
# Waiting is removed by SDK finish BEFORE callback; the consumed request remains.
test -f '@ROOT@/input-observation-end' && test ! -L '@ROOT@/input-observation-end' || exit 90
test "$(stat -c '%a %u' '@ROOT@/input-observation-end')" = '600 0'
test "$(wc -c < '@ROOT@/input-observation-end')" -le 256
read n wp ws dev ino stage setup profile extra < '@ROOT@/input-observation-end'
test -z "$extra" && test "$n" = '@NONCE@' && test "$wp $ws" = "$p $started" || exit 90
case "$dev:$ino" in *[!0-9:]*|:*|*:|*::*) exit 90;; esac
test "$stage" = end-input-observation && test "$setup" = 120000 && test "$profile" = main-dev-input-observation-120s || exit 90
test "$(cat '@ROOT@/input-observation-end')" = "$n $wp $ws $dev $ino $stage $setup $profile"
test -d '@ROOT@' && test ! -L '@ROOT@' || exit 90
test "$(stat -c '%d %i %a %u' '@ROOT@')" = "$dev $ino 700 0"
test "$(cat '@ROOT@/owner')" = '@NONCE@'
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(sha256sum "/proc/$p/exe" | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
awk -v so='@ROOT@/payload.so' '$NF==so {found=1} END {exit !found}' "/proc/$p/maps"
test "$(sha256sum '@ROOT@/payload.so' | awk '{print $1}')" = '@PAYLOADHASH@'
env_records=$(set -o pipefail || exit 90; cat "/proc/$p/environ" | LC_ALL=C tr '\n' '\r' | LC_ALL=C tr '\000' '\n') || exit 90
printf '%s\n' "$env_records" | LC_ALL=C awk -v so='@ROOT@/payload.so' '$0=="LD_PRELOAD="so {found=1} END {exit !found}'
test -z "$(systemctl show --property=Job --value xochitl.service)"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
printf '%s %s %s %s\n' "$p" "$started" "$dev" "$ino"
'@
            $identity=ObservationSSH $identityCommand
            Require $identity
            if($identity.stdout -cnotmatch '^([1-9][0-9]*) ([1-9][0-9]*) ([0-9]+) ([0-9]+)\n$'){throw 'Live process tuple refused'}
            $livePid=[long]$Matches[1];$liveStart=$Matches[2]
            $liveDev=$Matches[3];$liveIno=$Matches[4]
            $completion=$observed.stdout|ConvertFrom-Json
            if(-not(Test-InputObservationCompletion $completion $nonce ([string]$livePid) $liveStart $liveDev $liveIno)){throw 'GUI completion proof refused'}
            $record.gui_completion_verified=$true
            $record.observation_completion=$completion
            [IO.File]::WriteAllText((Join-Path $packet 'input-observation-complete.json'),$observed.stdout,[Text.UTF8Encoding]::new($false))
            $record.gui_completion_observed_ms=$observationClock.ElapsedMilliseconds
            RequireSameObservationIdentity
            $record.observation_generation_verified=$true
            if($developmentEvdevLogging){
                try{
                    $endpoint=ObservationSSH $InputEvdevSnapshotCommand
                    if(-not $endpoint.timeout -and $endpoint.exit -eq 0){$evdevEnd=Get-InputEvdevEndpoint $endpoint.stdout}
                }catch{$evdevEndpointReason='endpoint-refused-or-original-deadline'}
            }
            if($completion.image_status -ceq 'available'){
                $imageCheck=ObservationSSH (Expand "set -eu; test -f '@ROOT@/input-window.png'; test ! -L '@ROOT@/input-window.png'; test `"`$(stat -c '%a %u' '@ROOT@/input-window.png')`" = '600 0'; test `"`$(wc -c < '@ROOT@/input-window.png')`" = '$($completion.png_bytes)'; sha256sum '@ROOT@/input-window.png'")
                Require $imageCheck
                if($imageCheck.stdout -cnotmatch ('^([0-9a-f]{64})  '+[regex]::Escape($remote+'/input-window.png')+'\n$')){throw 'Qt image hash refused'}
                $qtHash=$Matches[1];$qtImagePath=Join-Path $packet 'input-window.png'
                Require (ObservationCopy ($remote+'/input-window.png') $qtImagePath)
                if((Hash $qtImagePath) -cne $qtHash){throw 'Qt image bytes changed'}
                $record.qt_saved_copy_verified=$true;$record.qt_saved_copy_sha256=$qtHash
                RequireSameObservationIdentity
                $record.qt_image_available=$true;$record.qt_image_sha256=$qtHash
            }
            # ONE existing heap capture, after GUI completion and before finally.
            # Transport timeout cannot hard-cancel remote capture; restoration remains owed.
            $heapRemote='/tmp/rem25-facts-'+$nonce+'-input-observation.png'
            RequireSameObservationIdentity
            $record.heap_capture_start_ms=$observationClock.ElapsedMilliseconds
            $record.heap_capture_transport_unknown=$true
            $capture=ObservationSSH ("set -eu; test -f /home/root/rem9-validation/screenshot; test ! -L /home/root/rem9-validation/screenshot; test `"`$(sha256sum /home/root/rem9-validation/screenshot | awk '{print `$1}')`" = 'df745d56a2ef1834ea644b8859646972210c5e0247a3e6e75e9e245f5fa919b8'; test ! -e '$heapRemote'; test ! -L '$heapRemote'; /home/root/rem9-validation/screenshot '$heapRemote' > /dev/null; test -f '$heapRemote'; test ! -L '$heapRemote'; test `"`$(wc -c < '$heapRemote')`" -le 8388608; sha256sum '$heapRemote'")
            $record.heap_capture_end_ms=$observationClock.ElapsedMilliseconds
            Require $capture
            if($capture.stdout.Length -gt 256 -or $capture.stdout -cnotmatch ('^([0-9a-f]{64})  '+[regex]::Escape($heapRemote)+'\n$')){throw 'Heap image hash refused'}
            $heapHash=$Matches[1]
            $record.heap_capture_transport_unknown=$false
            RequireSameObservationIdentity
            $heapImagePath=Join-Path $packet 'heap-after-gui.png'
            Require (ObservationCopy $heapRemote $heapImagePath)
            if((Hash $heapImagePath) -cne $heapHash -or -not(Test-InputObservationLegacyOverviewPng $heapImagePath 'legacy-df745-overview-768x1024')){throw 'Heap image bytes/legacy overview profile refused'}
            $record.heap_saved_copy_verified=$true;$record.heap_saved_copy_sha256=$heapHash
            RequireSameObservationIdentity
            $record.heap_capture_succeeded=$true;$record.heap_image_sha256=$heapHash
            $record.paired_image_candidate_verified=$record.qt_image_available
            $record.image_coherence_verified=$false
            $record.live_observation_ms=$observationClock.ElapsedMilliseconds
            break
        }
        if($observed.timeout){
            if($statusTimeoutUsed){throw 'Facts observation transport unknown'}
            $statusTimeoutUsed=$true
            continue
        }
        if($observed.exit -ne 3){throw 'Facts callback observation refused'}
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
            # Historical observation evidence only; no qualification flags change.
            foreach($name in @('input-observation-ready','input-observation-end','input-observation-complete.json')){
                $cap=if($name -ceq 'input-observation-complete.json'){8192}else{256}
                $historical=SSH ("set -eu; if test -f '$remote/$name'; then test ! -L '$remote/$name'; test `"`$(stat -c '%a %u' '$remote/$name')`" = '600 0'; test `"`$(wc -c < '$remote/$name')`" -le $cap; cat '$remote/$name'; else test ! -e '$remote/$name'; test ! -L '$remote/$name'; fi")
                Require $historical
                if($historical.stdout){[IO.File]::WriteAllText((Join-Path $packet ('historical-'+$name)),$historical.stdout,[Text.UTF8Encoding]::new($false))}
            }
            foreach($name in @('restore.failure','verification.first-refusal')){
                $diagnostic=SSH ("if test -f '$remote/$name'; then test `"`$(wc -c < '$remote/$name')`" -le 128 && cat '$remote/$name'; fi")
                Require $diagnostic
                if($diagnostic.stdout){[IO.File]::WriteAllText((Join-Path $packet $name),$diagnostic.stdout,[Text.UTF8Encoding]::new($false))}
            }
            $imagesPreserved=PreserveObservationImages
            if($imagesPreserved -and -not $record.heap_capture_transport_unknown){
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
for name in payload.so launch.sh restore.sh native-probe.conf owner dropin.sha256 callback.json attempt.claim attempt.identity restore.claim restored parent.signature admission.lock entry.closed restore.failure verification.first-refusal diagnostics.json refusal.json input-observation-end-publisher check-input-observation-ready.sh publish-input-observation-end-once.sh facts-waiting facts-request facts-request.tmp input-observation-ready input-observation-end input-observation-end.tmp input-observation-complete.json input-window.png; do rm -f '@ROOT@/'"$name"; done
@HEAPCLEANUP@
rmdir '@ROOT@'
test ! -e '@ROOT@'
systemctl is-active xochitl.service reader-buddy.service rm-sync.service
printf 'stock-restored-and-exact-stage-removed\n'
'@)
            $record.cleanup_verified=-not $cleaned.timeout -and $cleaned.exit -eq 0 -and -not $record.heap_capture_transport_unknown
            }else{
                $record.cleanup_verified=$false
            }
        }
    }
    if($developmentEvdevLogging -and $record.restored){
        try{
            $window=Get-InputEvdevWindow $evdevBegin $evdevEnd
            $diagnostic=@{diagnostic_status='unknown';reason=$evdevEndpointReason;records=@();acquisition_stage='not-attempted'}
            if($window -and $record.observation_completion){
                $journalCommand=Get-InputEvdevJournalCommand $window ([string]$record.observation_completion.attempt_pid) $nonce
                $capture=Invoke-InputEvdevCapture 'ssh' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=2','RM2',$journalCommand.Replace("`r`n","`n"))
                $diagnostic=Convert-InputEvdevJournal $capture $window ([string]$record.observation_completion.attempt_pid) $nonce
                $diagnostic.acquisition_stage='attempted'
                $diagnostic.transport=@{exit=$capture.exit;timeout=$capture.timeout;stdout_bytes=$capture.stdout_bytes.Length;stderr_bytes=$capture.stderr_bytes.Length;stderr_base64=[Convert]::ToBase64String($capture.stderr_bytes);stdout_overflow=$capture.stdout_overflow;stderr_overflow=$capture.stderr_overflow;acquisition_error=$capture.acquisition_error;elapsed_ms=$capture.elapsed_ms}
                $diagnostic.query_command=$journalCommand
            }
            $diagnostic.nonce=$nonce;$diagnostic.begin=$evdevBegin;$diagnostic.end=$evdevEnd;$diagnostic.window=$window
            $diagnostic.candidate_tuple=$record.observation_completion|Select-Object attempt_pid,attempt_start,root_device,root_inode
            $diagnostic.category_provenance='explicit prefix only; point coverage unqualified'
            $diagnosticPath=Join-Path $packet 'evdev-journal-diagnostic.json'
            [IO.File]::WriteAllText($diagnosticPath,($diagnostic|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
            $record.evdev_journal=@{diagnostic_status=$diagnostic.diagnostic_status;sha256=(Hash $diagnosticPath)}
        }catch{$record.evdev_journal=@{diagnostic_status='unknown';reason='diagnostic-acquisition-or-persistence-failed'}}
    }
    }finally{
    [IO.File]::WriteAllText((Join-Path $packet 'operator-receipt.json'),($record|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
    Write-Output ('receipt='+ (Join-Path $packet 'operator-receipt.json'))
    Write-Output ('restored='+$record.restored+' cleanup_verified='+$record.cleanup_verified)
    }
    if(-not $record.cleanup_verified){throw 'Restoration/stage uncertain; preserve exact path and timer duty, no retry'}

    if(-not $record.gui_completion_verified){throw 'Stock restored; GUI completion proof refused or incomplete'}
}
