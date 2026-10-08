param([Parameter(Mandatory=$true)][string]$PayloadPath,
      [Parameter(Mandatory=$true)][string]$PublisherPath,
      [Parameter(Mandatory=$true)][string]$CapturePublisherPath,
      [Parameter(Mandatory=$true)][string]$BuildConfigPath,
      [Parameter(Mandatory=$true)][string]$ExpectedPath,
      [Parameter(Mandatory=$true)][string]$StockBaselinePath,
      [Parameter(Mandatory=$true)][string]$EvidenceDirectory, [switch]$PrepareOnly)
$ErrorActionPreference='Stop'
throw 'SOURCE ONLY: requires a separately selected fresh nonce, artifacts, baseline and exact packet review; spent literals below are reference placeholders.'
$nonce='b3e3ca0475a84432a9b328218395de0c'
$remote='/run/rmb-qt-probe-'+$nonce
$rollback='rmb-qt-probe-'+$nonce+'-rollback'
$payloadHash='0000000000000000000000000000000000000000000000000000000000000000'
$publisherHash='0000000000000000000000000000000000000000000000000000000000000000'
$capturePublisherHash='0000000000000000000000000000000000000000000000000000000000000000'
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
. $budgetPath
. "$PSScriptRoot/capture-observation-proof.ps1"
. "$PSScriptRoot/capture-observation-collector.ps1"
. "$PSScriptRoot/capture-observation-build-config.ps1"
$budget=Get-FactsDevelopmentBudget
if((Hash $PayloadPath) -cne $payloadHash -or (Hash $PublisherPath) -cne $publisherHash -or (Hash $ExpectedPath) -cne $expectedHash -or (Hash $StockBaselinePath) -cne $stockBaselineHash){throw 'Frozen candidate input changed'}
$expected=Get-Content -LiteralPath $ExpectedPath -Raw|ConvertFrom-Json
$stock=Get-Content -LiteralPath $StockBaselinePath -Raw|ConvertFrom-Json
if((Hash $CapturePublisherPath) -cne $capturePublisherHash -or [IO.File]::ReadAllText($BuildConfigPath).Replace("`r`n","`n") -cne (Get-CaptureObservationBuildConfig $expected)){throw 'Capture publisher/build option binding refused'}
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
foreach($name in @('launch.sh','restore.sh','native-probe.conf','check-waiting.sh','publish-capture-observation-source.sh')){
    $files[$name]=Freeze $name ([IO.File]::ReadAllText((Join-Path $PSScriptRoot $name)).Replace("`r`n","`n"))
}
$files.owner=Freeze 'owner' $nonce
$files['dropin.sha256']=Freeze 'dropin.sha256' $files['native-probe.conf']
# Final facts wrapper is a separately reviewed Main recipe after complete-image
# review, with a literal local visual SHA. It is not pre-enabled/transferred here.
$factsRecipeTemplateHash=Hash (Join-Path $PSScriptRoot 'publish-captured-facts-source.sh')
$localBindings=[ordered]@{nonce=$nonce;operator_sha256=(Hash $PSCommandPath);stock_baseline_sha256=(Hash $StockBaselinePath);
 expected_sha256=$expectedHash;payload_sha256=$payloadHash;publisher_sha256=$publisherHash;proof_sha256=(Hash $proofPath);budget_sha256=(Hash $budgetPath);capture_publisher_sha256=$capturePublisherHash;build_config_sha256=(Hash $BuildConfigPath);sdk_source='f0e6ff4ccb37b887f7f278b0b820a7d047de1559';developmentCaptureObservation=$true;facts_recipe_template_sha256=$factsRecipeTemplateHash;capture_proof_sha256=(Hash (Join-Path $PSScriptRoot 'capture-observation-proof.ps1'));capture_collector_sha256=(Hash (Join-Path $PSScriptRoot 'capture-observation-collector.ps1'));files=$files}
[void](Freeze 'packet-bindings.json' ($localBindings|ConvertTo-Json -Depth 5))
if($PrepareOnly){$localBindings|ConvertTo-Json -Depth 5;return}
# Main alone executes after independent artifact/operator review and advance notice.
$record=[ordered]@{nonce=$nonce;experiment='development-capture-before-facts';payload_source='f0e6ff4ccb37b887f7f278b0b820a7d047de1559';publisher_source='unselected-consumer-source-checkpoint';bindings=$localBindings;budget=$budget;results=@();arm_intent=$false;armed=$false;callback_verified=$false;candidate_generation_verified=$false;facts_verified=$false;candidate_receipt=$null;restored=$false;cleanup_verified=$false;live_diagnostics_collected=$false;live_diagnostics_sha256=$null;gate_evidence=@{};diagnostic_collected=$false;diagnostic_sha256=$null;final_callback_state='not-checked';final_callback_collected=$false;final_callback_sha256=$null;live_observation_ms=$null;capture_verified=$false;capture_completion_saved_copy_verified=$false;capture_png_saved_copy_verified=$false;capture_request_saved_copy_verified=$false;capture_visual_saved_copy_verified=$false;capture_visual_saved_copy_sha256=$null;live_refusal=$null;live_refusal_callback_match=$false;final_refusal=$null;final_refusal_callback_match=$false}
function Native([string]$program,[string[]]$arguments,[int]$timeoutMs=10000){
    if($program -ceq 'scp'){$timeoutMs=[int][Math]::Min(5000,$timeoutMs)}
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
    if($remaining -le 0){throw 'Capture copy budget exhausted; restoration required'}
    $result=Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=2',('RM2:'+$remotePath),$localPath) ([int][Math]::Min(5000,$remaining))
    # Caller records verified returned bytes before its next fresh live-clock guard.
    return $result
}
function Expand([string]$text){$text.Replace('@ROOT@',$remote).Replace('@NONCE@',$nonce).Replace('@UNIT@',$rollback).Replace('@STOCKPID@',[string]$stock.stock_pid).Replace('@STOCKSTART@',$stock.stock_start).Replace('@FIXTURECHECK@',$fixtureCheck)}
try{
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
if awk 'BEGIN {RS="\0"} /^(LD_PRELOAD|LD_LIBRARY_PATH|XOVI_ROOT)=/ {found=1} END {exit !found}' /proc/@STOCKPID@/environ; then exit 90; fi
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
    Require (Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',$PublisherPath,('RM2:'+$remote+'/facts-publisher')))
    Require (SSH ("set -eu; test `"`$(sha256sum '$remote/facts-publisher' | awk '{print `$1}')`" = '$publisherHash'; chmod 700 '$remote/facts-publisher'"))
    Require (Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',$CapturePublisherPath,('RM2:'+$remote+'/capture-publisher')))
    Require (SSH ("set -eu; test `"`$(sha256sum '$remote/capture-publisher' | awk '{print `$1}')`" = '$capturePublisherHash'; chmod 700 '$remote/capture-publisher'"))
    foreach($name in $files.Keys){Require (SSH ("set -eu; test `"`$(sha256sum '$remote/$name' | awk '{print `$1}')`" = '$($files[$name])'; chmod 600 '$remote/$name'"))}
    # Observation admission budget starts before arming. Recovery and final
    # evidence collection keep their separate mandatory duty after this cutoff.
    $observationClock=[Diagnostics.Stopwatch]::StartNew()
    $record.arm_intent=$true
    Require (SSH (Expand @'
set -eu
test "$(sha256sum '@ROOT@/payload.so' | awk '{print $1}')" = a747186fb466b8947caf46a9629654f24743f090b0f7f3e27e764e2dbacdc226
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
    # Main performs positive waiting proof, ONE preselected90,260/raw164,1396 input
    # with BOTH axes fresh and full release, then capture publication once and
    # full image review before the selected facts publication. No heap fallback.
    # Main alone publishes; collector only reads/copies under original clocks.
    # Both purpose-specific publications are Main-owned, once each.
    # Read-only observations are finite; no setup sleeps or physical retries.
    $statusTimeoutUsed=$false
    while(Test-FactsLiveObservationWindow $observationClock.ElapsedMilliseconds){
        [void](Receive-CaptureObservation $remote $nonce $payloadHash $expected $packet $record {param($command)ObservationSSH $command} {param($remotePath,$localPath)ObservationCopy $remotePath $localPath})
        if($record.capture_verified -and $record.capture_completion.page_index -ne 0){throw 'Selected first fixture page not captured; no facts publication'}
        $observed=ObservationSSH (Expand @'
set -eu
test -d '@ROOT@' && test ! -L '@ROOT@' || exit 90
test "$(stat -c '%a %u' '@ROOT@')" = '700 0' || exit 90
test "$(cat '@ROOT@/owner')" = '@NONCE@' || exit 90
test ! -e '@ROOT@/entry.closed' && test ! -L '@ROOT@/entry.closed' || exit 90
test ! -e '@ROOT@/restore.claim' && test ! -L '@ROOT@/restore.claim' || exit 90
if test ! -e '@ROOT@/callback.json' && test ! -L '@ROOT@/callback.json'; then exit 3; fi
test -f '@ROOT@/callback.json' && test ! -L '@ROOT@/callback.json' || exit 90
test "$(stat -c '%a %u' '@ROOT@/callback.json')" = '600 0' || exit 90
test "$(wc -c < '@ROOT@/callback.json')" -le 256 || exit 90
cat '@ROOT@/callback.json'
'@)
        if(-not $observed.timeout -and $observed.exit -eq 0){
            [IO.File]::WriteAllText((Join-Path $packet 'callback.json'),$observed.stdout,[Text.UTF8Encoding]::new($false))
            $callback=$observed.stdout|ConvertFrom-Json
            $record.callback_verified=Test-FactsCallback $callback $nonce
            $record.candidate_receipt=$callback
            $identity=ObservationSSH (Expand @'
set -eu
test ! -e '@ROOT@/entry.closed'
test ! -e '@ROOT@/restore.claim'
test -f '@ROOT@/attempt.identity' && test ! -L '@ROOT@/attempt.identity' || exit 90
test "$(stat -c '%a %u' '@ROOT@/attempt.identity')" = '600 0'
test "$(wc -c < '@ROOT@/attempt.identity')" -le 128
read p started < '@ROOT@/attempt.identity'
case "$p:$started" in *[!0-9:]*|:*|*:) exit 90;; esac
# Waiting is removed by SDK finish BEFORE callback; the consumed request remains.
test -f '@ROOT@/facts-request' && test ! -L '@ROOT@/facts-request' || exit 90
test "$(stat -c '%a %u' '@ROOT@/facts-request')" = '600 0'
test "$(wc -c < '@ROOT@/facts-request')" -le 128
read n wp ws dev ino stage setup profile extra < '@ROOT@/facts-request'
test -z "$extra" && test "$n" = '@NONCE@' && test "$wp $ws" = "$p $started" || exit 90
case "$dev:$ino" in *[!0-9:]*|:*|*:|*::*) exit 90;; esac
test "$stage" = read-facts && test "$setup" = 120000 && test "$profile" = main-dev-facts-120s || exit 90
test "$(cat '@ROOT@/facts-request')" = "$n $wp $ws $dev $ino $stage $setup $profile"
test -d '@ROOT@' && test ! -L '@ROOT@' || exit 90
test "$(stat -c '%d %i %a %u' '@ROOT@')" = "$dev $ino 700 0"
test "$(cat '@ROOT@/owner')" = '@NONCE@'
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(sha256sum "/proc/$p/exe" | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
awk -v so='@ROOT@/payload.so' '$NF==so {found=1} END {exit !found}' "/proc/$p/maps"
test -z "$(systemctl show --property=Job --value xochitl.service)"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
printf '%s %s %s %s\n' "$p" "$started" "$dev" "$ino"
'@)
            Require $identity
            if($identity.stdout -cnotmatch '^([1-9][0-9]*) ([1-9][0-9]*) ([0-9]+) ([0-9]+)\n$'){throw 'Live process tuple refused'}
            $livePid=[long]$Matches[1];$liveStart=$Matches[2]
            $liveDev=$Matches[3];$liveIno=$Matches[4]
            if(-not $record.callback_verified){
                $collectedRefusal=ObservationSSH (Expand (Get-FactsRefusalReadCommand))
                $savedRefusal=Save-FactsRefusalEvidence $collectedRefusal (Join-Path $packet 'refusal-live.json') $nonce $livePid $liveStart
                Require (ObservationSSH (Expand "set -eu; test -d '@ROOT@'; test ! -L '@ROOT@'; test `"`$(stat -c '%d %i %a %u' '@ROOT@')`" = '$liveDev $liveIno 700 0'; test `"`$(cat '@ROOT@/owner')`" = '@NONCE@'; test ! -e '@ROOT@/entry.closed'; test ! -L '@ROOT@/entry.closed'; test ! -e '@ROOT@/restore.claim'; test ! -L '@ROOT@/restore.claim'; test `"`$(systemctl show --property=MainPID --value xochitl.service)`" = '$livePid'; test `"`$(awk '{print `$22}' /proc/$livePid/stat)`" = '$liveStart'; test -z `"`$(systemctl show --property=Job --value xochitl.service)`""))
                if(-not(Test-FactsLiveObservationWindow $observationClock.ElapsedMilliseconds)){throw 'Refusal collection outside original live clock'}
                $record.live_refusal=$savedRefusal
                $record.live_refusal_callback_match=(Test-FactsRefusalCallback $callback $nonce) -and $savedRefusal.decoded -and $savedRefusal.evidence.entry_stage -ceq $callback.stage
                throw 'Facts read refused; bounded diagnostic evidence retained; restoration required'
            }
            $liveDiagnostics=ObservationSSH (Expand "set -eu; test -f '@ROOT@/diagnostics.json'; test ! -L '@ROOT@/diagnostics.json'; test `"`$(stat -c '%a %u' '@ROOT@/diagnostics.json')`" = '600 0'; test `"`$(wc -c < '@ROOT@/diagnostics.json')`" -le 8192; cat '@ROOT@/diagnostics.json'")
            Require $liveDiagnostics
            $liveDiagnosticsPath=Join-Path $packet 'live-diagnostics.json'
            [IO.File]::WriteAllText($liveDiagnosticsPath,$liveDiagnostics.stdout,[Text.UTF8Encoding]::new($false))
            $record.live_diagnostics_collected=$true
            $record.live_diagnostics_sha256=Hash $liveDiagnosticsPath
            $diagnostics=$liveDiagnostics.stdout|ConvertFrom-Json
            # Recheck the same live tuple/empty job after collecting; no historical upgrade.
            Require (ObservationSSH (Expand "set -eu; test -d '@ROOT@'; test ! -L '@ROOT@'; test `"`$(stat -c '%d %i %a %u' '@ROOT@')`" = '$liveDev $liveIno 700 0'; test `"`$(cat '@ROOT@/owner')`" = '@NONCE@'; test ! -e '@ROOT@/entry.closed'; test ! -L '@ROOT@/entry.closed'; test ! -e '@ROOT@/restore.claim'; test ! -L '@ROOT@/restore.claim'; test `"`$(systemctl show --property=MainPID --value xochitl.service)`" = '$livePid'; test `"`$(awk '{print `$22}' /proc/$livePid/stat)`" = '$liveStart'; test -z `"`$(systemctl show --property=Job --value xochitl.service)`""))
            $record.live_observation_ms=$observationClock.ElapsedMilliseconds
            if(-not(Test-FactsLiveObservationWindow $record.live_observation_ms)){throw 'Live facts proof outside original clock'}
            $record.candidate_generation_verified=$true
            $record.facts_verified=$record.callback_verified -and $record.capture_verified -and
                (Test-FactsDiagnostics $diagnostics $nonce $expected.document $expected.order $livePid $liveStart) -and
                $diagnostics.document_id -ceq $record.capture_completion.document_id -and
                $diagnostics.current_page_id -ceq $record.capture_completion.page_id -and
                $diagnostics.current_index -eq $record.capture_completion.page_index
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
            foreach($name in @('facts-waiting','facts-request')){
                $gate=SSH (Expand (@'
set -eu
if test -f '@ROOT@/@NAME@'; then
    test ! -L '@ROOT@/@NAME@'
    test "$(stat -c '%a %u' '@ROOT@/@NAME@')" = '600 0'
    test "$(stat -c '%s' '@ROOT@/@NAME@')" -le 128
    printf 'present\n'
    cat '@ROOT@/@NAME@'
else
    test ! -e '@ROOT@/@NAME@'
    test ! -L '@ROOT@/@NAME@'
    printf 'absent\n'
fi
'@).Replace('@NAME@',$name))
                Require $gate
                if($gate.stdout.StartsWith("present`n",[StringComparison]::Ordinal)){
                    $gatePath=Join-Path $packet $name
                    [IO.File]::WriteAllText($gatePath,$gate.stdout.Substring(8),[Text.UTF8Encoding]::new($false))
                    $record.gate_evidence[$name]=@{state='present';sha256=(Get-FileHash -LiteralPath $gatePath -Algorithm SHA256).Hash.ToLowerInvariant()}
                }elseif($gate.stdout -ceq "absent`n"){$record.gate_evidence[$name]=@{state='absent'}}
                else{throw 'Gate evidence uncertain; retain exact stage'}
            }
            foreach($name in @('attempt.claim','attempt.identity','restore.claim','restored','parent.signature','entry.closed')){
                $collected=Native 'scp' @('-o','HostName=10.11.99.1','-o','StrictHostKeyChecking=yes','-o','BatchMode=yes','-o','ConnectTimeout=8',('RM2:'+$remote+'/'+$name),(Join-Path $packet $name))
                Require $collected
            }
            # Historical refusal collection only; never update live/facts flags.
            $attemptText=[IO.File]::ReadAllText((Join-Path $packet 'attempt.identity'))
            if($attemptText -cnotmatch '^([1-9][0-9]*) ([1-9][0-9]*)\n$'){throw 'Final attempted identity refused'}
            $attemptPid=[long]$Matches[1];$attemptStart=$Matches[2]
            $finalTransport=SSH (Expand (Get-FactsRefusalReadCommand))
            $record.final_refusal=Save-FactsRefusalEvidence $finalTransport (Join-Path $packet 'refusal-final.json') $nonce $attemptPid $attemptStart
            $finalRefusalCallback=$null
            if($record.final_callback_collected){
                try{$finalRefusalCallback=Get-Content -LiteralPath (Join-Path $packet 'callback-final.json') -Raw|ConvertFrom-Json -ErrorAction Stop}catch{}
            }
            $record.final_refusal_callback_match=(Test-FactsRefusalCallback $finalRefusalCallback $nonce) -and $record.final_refusal.decoded -and $record.final_refusal.evidence.entry_stage -ceq $finalRefusalCallback.stage
            foreach($name in @('restore.failure','verification.first-refusal')){
                $diagnostic=SSH ("if test -f '$remote/$name'; then test `"`$(wc -c < '$remote/$name')`" -le 128 && cat '$remote/$name'; fi")
                Require $diagnostic
                if($diagnostic.stdout){[IO.File]::WriteAllText((Join-Path $packet $name),$diagnostic.stdout,[Text.UTF8Encoding]::new($false))}
            }
            $visualPath=Join-Path $packet 'capture-visual-review.json'
            if(Test-Path -LiteralPath $visualPath -PathType Leaf){
                if((Get-Item -LiteralPath $visualPath).Length -gt 4096){throw 'Visual record exceeds fixed cap'}
                $record.capture_visual_saved_copy_sha256=Hash $visualPath;$record.capture_visual_saved_copy_verified=$true
            }
            $finalRecipePath=Join-Path $packet 'publish-captured-facts-source.sh'
            if(Test-Path -LiteralPath $finalRecipePath -PathType Leaf){
                if((Get-Item -LiteralPath $finalRecipePath).Length -gt 4096){throw 'Final facts recipe exceeds fixed cap'}
                $record.capture_facts_recipe_saved_copy_sha256=Hash $finalRecipePath;$record.capture_facts_recipe_saved_copy_verified=$true
            }
            if(-not(Test-CapturePreserved $remote $packet $record {param($command)SSH $command})){throw 'Capture outputs unpreserved; retain exact stage after stock restoration'}
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
for name in payload.so launch.sh restore.sh native-probe.conf owner dropin.sha256 callback.json attempt.claim attempt.identity restore.claim restored parent.signature admission.lock entry.closed restore.failure verification.first-refusal diagnostics.json refusal.json facts-publisher capture-publisher check-waiting.sh publish-captured-facts-source.sh publish-capture-observation-source.sh facts-waiting facts-request facts-request.tmp capture-observation-request capture-observation-request.tmp capture-observation-complete.json capture-window.png capture-visual-review.json; do rm -f '@ROOT@/'"$name"; done
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
    if(-not $record.facts_verified){throw 'Stock restored; fixed facts proof refused or incomplete'}
}
