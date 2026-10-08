# Fixed development collection helpers. Main supplies bounded existing transports.
function Receive-CaptureObservation([string]$Root,[string]$Nonce,[string]$PayloadSha,$Expected,[string]$Packet,[Collections.IDictionary]$Record,[scriptblock]$Read,[scriptblock]$Copy) {
    if($Nonce -cnotmatch '\A[0-9a-f]{32}\z' -or $Root -cne ('/run/rmb-qt-probe-'+$Nonce) -or $PayloadSha -cnotmatch '\A[0-9a-f]{64}\z'){throw 'Capture bindings refused'}
    if($Record.capture_verified){return $true}
    $status=& $Read ("set -eu; if test ! -e '$Root/capture-observation-complete.json' && test ! -L '$Root/capture-observation-complete.json'; then exit 3; fi; test -f '$Root/capture-observation-complete.json' && test ! -L '$Root/capture-observation-complete.json'; test `"`$(stat -c '%a %u' '$Root/capture-observation-complete.json')`" = '600 0'; test `"`$(wc -c < '$Root/capture-observation-complete.json')`" -le 8192; cat '$Root/capture-observation-complete.json'")
    if(-not $status.timeout -and $status.exit -eq 3){return $false}
    if($status.timeout -or $status.exit -ne 0){throw 'Capture status unknown; retain outputs and restoration duty'}
    $identityCommand=@'
set -eu
root='@ROOT@'
test -d "$root" && test ! -L "$root"
test "$(stat -c '%a %u' "$root")" = '700 0'
test "$(cat "$root/owner")" = '@NONCE@'
for name in entry.closed restore.claim callback.json facts-request facts-request.tmp; do test ! -e "$root/$name" && test ! -L "$root/$name"; done
test -f "$root/attempt.identity" && test ! -L "$root/attempt.identity"
test "$(stat -c '%a %u' "$root/attempt.identity")" = '600 0'
test "$(wc -c < "$root/attempt.identity")" -le 128
read p started < "$root/attempt.identity"
case "$p:$started" in *[!0-9:]*|:*|*:) exit 90;; esac
test "$p" -gt 1 && test "$started" -gt 0
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
test -z "$(systemctl show --property=Job --value xochitl.service)"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(sha256sum "/proc/$p/exe" | awk '{print $1}')" = 071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df
test "$(sha256sum "$root/payload.so" | awk '{print $1}')" = '@PAYLOAD@'
awk -v so="$root/payload.so" '$NF==so {found=1} END {exit !found}' "/proc/$p/maps"
test "$(awk '{print $22}' "/proc/$p/stat")" = "$started"
test "$(systemctl show --property=MainPID --value xochitl.service)" = "$p"
printf '%s %s ' "$p" "$started"
stat -c '%d %i' "$root"
'@
    $identityCommand=$identityCommand.Replace('@ROOT@',$Root).Replace('@NONCE@',$Nonce).Replace('@PAYLOAD@',$PayloadSha)
    $before=& $Read $identityCommand
    if($before.timeout -or $before.exit -ne 0 -or $before.stdout -cnotmatch '\A([1-9][0-9]{0,19}) ([1-9][0-9]{0,19}) ([1-9][0-9]{0,19}) ([1-9][0-9]{0,19})\n\z'){throw 'Capture candidate unknown'}
    $identity=@($Matches[1],$Matches[2],$Matches[3],$Matches[4])
    $value=$status.stdout|ConvertFrom-Json
    if(-not(Test-CaptureObservationCompletion $value $Nonce $identity[0] $identity[1] $identity[2] $identity[3] $Expected.document $Expected.order)){throw 'Capture completion refused'}
    $request=& $Read ("set -eu; test -f '$Root/capture-observation-request' && test ! -L '$Root/capture-observation-request'; test `"`$(stat -c '%a %u' '$Root/capture-observation-request')`" = '600 0'; test `"`$(wc -c < '$Root/capture-observation-request')`" -le 256; cat '$Root/capture-observation-request'")
    $requestBytes=$Nonce+' '+($identity -join ' ')+" capture-observation 120000 main-dev-facts-120s`n"
    if($request.timeout -or $request.exit -ne 0 -or $request.stdout -cne $requestBytes){throw 'Capture request binding refused'}
    $requestPath=Join-Path $Packet 'capture-observation-request'
    if(Test-Path -LiteralPath $requestPath){throw 'Capture saved request already exists'}
    [IO.File]::WriteAllText($requestPath,$request.stdout,[Text.UTF8Encoding]::new($false))
    $Record.capture_request_saved_copy_sha256=(Get-FileHash -LiteralPath $requestPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $Record.capture_request_saved_copy_verified=$true
    $completionPath=Join-Path $Packet 'capture-observation-complete.json'
    if(Test-Path -LiteralPath $completionPath){throw 'Capture saved completion already exists'}
    [IO.File]::WriteAllText($completionPath,$status.stdout,[Text.UTF8Encoding]::new($false))
    $Record.capture_completion_saved_copy_sha256=(Get-FileHash -LiteralPath $completionPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $Record.capture_completion_saved_copy_verified=$true
    $imagePath=Join-Path $Packet 'capture-window.png'
    if(Test-Path -LiteralPath $imagePath){throw 'Capture saved image already exists'}
    $copied=& $Copy ($Root+'/capture-window.png') $imagePath
    if($copied.timeout -or $copied.exit -ne 0 -or -not(Test-CaptureObservationImage $value $imagePath)){throw 'Capture image unknown; preserve exact output'}
    # Preservation knowledge survives later live refusal.
    $Record.capture_png_saved_copy_sha256=$value.png_sha256;$Record.capture_png_saved_copy_verified=$true
    $Record.capture_completion=$value
    $after=& $Read $identityCommand
    if($after.timeout -or $after.exit -ne 0 -or $after.stdout -cne $before.stdout){throw 'Capture candidate changed after copying'}
    $fresh=& $Read ("set -eu; test -f '$Root/capture-observation-complete.json' && test ! -L '$Root/capture-observation-complete.json'; test `"`$(stat -c '%a %u' '$Root/capture-observation-complete.json')`" = '600 0'; test `"`$(wc -c < '$Root/capture-observation-complete.json')`" -le 8192; cat '$Root/capture-observation-complete.json'")
    if($fresh.timeout -or $fresh.exit -ne 0 -or $fresh.stdout -cne $status.stdout){throw 'Capture completion changed after copying'}
    $final=& $Read $identityCommand
    if($final.timeout -or $final.exit -ne 0 -or $final.stdout -cne $before.stdout){throw 'Capture candidate changed after final binding'}
    $Record.capture_verified=$true
    return $true
}
function New-CaptureVisualReview($Completion,[string]$CompletionSha,[string]$RequestSha,[bool]$OpenedFixture) {
    # Main calls explicitly after reviewing the complete decoded image. This
    # function records that decision; it cannot infer or authenticate human vision.
    foreach($sha in @($CompletionSha,$RequestSha)){if($sha -cnotmatch '\A[0-9a-f]{64}\z'){throw 'Visual binding refused'}}
    [pscustomobject][ordered]@{kind='development-capture-visual-review';version=1;nonce=$Completion.nonce;attempt_pid=$Completion.attempt_pid;attempt_start=$Completion.attempt_start;root_device=$Completion.root_device;root_inode=$Completion.root_inode;document_id=$Completion.document_id;page_id=$Completion.page_id;page_index=$Completion.page_index;completion_sha256=$CompletionSha;png_sha256=$Completion.png_sha256;request_sha256=$RequestSha;visual_open_fixture=$OpenedFixture;reviewer='Main'}
}
function Test-CapturePreserved([string]$Root,[string]$Packet,[Collections.IDictionary]$Record,[scriptblock]$Read) {
    if($Root -cnotmatch '\A/run/rmb-qt-probe-[0-9a-f]{32}\z'){return $false}
    foreach($item in @(
        @{name='capture-window.png';local='capture-window.png';cap=8388608;flag='capture_png_saved_copy_verified';sha='capture_png_saved_copy_sha256'},
        @{name='capture-observation-complete.json';local='capture-observation-complete.json';cap=8192;flag='capture_completion_saved_copy_verified';sha='capture_completion_saved_copy_sha256'},
        @{name='capture-observation-request';local='capture-observation-request';cap=256;flag='capture_request_saved_copy_verified';sha='capture_request_saved_copy_sha256'},
        @{name='capture-observation-request.tmp';local='capture-observation-request';cap=256;flag='capture_request_saved_copy_verified';sha='capture_request_saved_copy_sha256'},
        @{name='capture-visual-review.json';local='capture-visual-review.json';cap=4096;flag='capture_visual_saved_copy_verified';sha='capture_visual_saved_copy_sha256'},
        @{name='publish-captured-facts-source.sh';local='publish-captured-facts-source.sh';cap=4096;flag='capture_facts_recipe_saved_copy_verified';sha='capture_facts_recipe_saved_copy_sha256'})){
        $remote=$Root+'/'+$item.name
        $metadata=& $Read ("set -eu; if test ! -e '$remote' && test ! -L '$remote'; then printf 'absent\n'; else test -f '$remote' && test ! -L '$remote'; test `"`$(stat -c '%a %u' '$remote')`" = '600 0'; bytes=`$(wc -c < '$remote'); test `"`$bytes`" -le '$($item.cap)'; hash=`$(sha256sum '$remote' | awk '{print `$1}'); printf 'present %s %s\n' `"`$bytes`" `"`$hash`"; fi")
        if($metadata.timeout -or $metadata.exit -ne 0){return $false}
        if($metadata.stdout -ceq "absent`n"){continue}
        if($metadata.stdout -cnotmatch '\Apresent ([1-9][0-9]{0,6}) ([0-9a-f]{64})\n\z'){return $false}
        $bytes=[long]$Matches[1];$sha=$Matches[2];$path=Join-Path $Packet $item.local
        if(-not $Record[$item.flag] -or $Record[$item.sha] -cne $sha -or -not(Test-Path -LiteralPath $path -PathType Leaf) -or (Get-Item -LiteralPath $path).Length -ne $bytes -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $sha){return $false}
    }
    return $true
}
