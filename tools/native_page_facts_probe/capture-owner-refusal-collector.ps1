# Historical collection only; Main supplies bounded transports and restore guards.
. "$PSScriptRoot/capture-historical-transport.ps1"
function Receive-CaptureOwnerRefusalEvidence([string]$Root,[string]$Nonce,$Identity,[string]$Packet,[Collections.IDictionary]$Record,[scriptblock]$Read,[scriptblock]$Copy,[bool]$FocusAncestry=$false,[bool]$ReceiverSubtreeCapture=$false,[bool]$ReceiverSubtreeCapture512=$false,[int]$ReceiverSubtreeItemCap=0) {
    if($Nonce -cnotmatch '\A[0-9a-f]{32}\z' -or $Root -cne ('/run/rmb-qt-probe-'+$Nonce)){throw 'Owner refusal root refused'}
    foreach($name in @('attempt_pid','attempt_start','root_device','root_inode')){
        $parsed=[ulong]0
        if($Identity.$name -isnot [string] -or $Identity.$name -cnotmatch '\A[1-9][0-9]{0,19}\z' -or -not [ulong]::TryParse($Identity.$name,[ref]$parsed)){throw 'Original owner identity refused'}
    }
    if($Record.capture_owner_refusal_saved_copy_verified){throw 'Owner refusal already collected; no implicit rebind'}
    $guard=@'
set -eu
root='@ROOT@'
test -d "$root" && test ! -L "$root"
test "$(stat -c '%d %i %a %u' "$root")" = '@DEV@ @INO@ 700 0'
test -f "$root/owner" && test ! -L "$root/owner"
test "$(stat -c '%a %u' "$root/owner")" = '600 0'
test "$(wc -c < "$root/owner")" = 32
test "$(cat "$root/owner")" = '@NONCE@'
test -f "$root/attempt.identity" && test ! -L "$root/attempt.identity"
test "$(stat -c '%a %u' "$root/attempt.identity")" = '600 0'
test "$(wc -c < "$root/attempt.identity")" -le 128
test "$(cat "$root/attempt.identity")" = '@PID@ @START@'
'@
    $guard=$guard.Replace('@ROOT@',$Root).Replace('@NONCE@',$Nonce).Replace('@DEV@',$Identity.root_device).Replace('@INO@',$Identity.root_inode).Replace('@PID@',$Identity.attempt_pid).Replace('@START@',$Identity.attempt_start)
    $metadata=@'
if test ! -e "$root/capture-owner-refusal.json" && test ! -L "$root/capture-owner-refusal.json"; then
 printf 'absent\n'
else
 test -f "$root/capture-owner-refusal.json" && test ! -L "$root/capture-owner-refusal.json"
 test "$(stat -c '%a %u' "$root/capture-owner-refusal.json")" = '600 0'
 bytes=$(wc -c < "$root/capture-owner-refusal.json"); test "$bytes" -le 8192
 hash=$(sha256sum "$root/capture-owner-refusal.json" | awk '{print $1}')
 printf 'present %s %s ' "$bytes" "$hash"
 stat -c '%d %i' "$root/capture-owner-refusal.json"
fi
'@
    $command=$guard+"`n"+$metadata+"`n"+$guard
    $before=& $Read $command
    if($before.timeout -or $before.exit -ne 0){throw 'Owner refusal output unknown; retain stage'}
    if($before.stdout -ceq "absent`n"){$Record.capture_owner_refusal_state='absent';$Record.capture_owner_refusal_decoded=$false;return}
    if($before.stdout -cnotmatch '\Apresent (0|[1-9][0-9]{0,3}) ([0-9a-f]{64}) ([1-9][0-9]{0,19}) ([1-9][0-9]{0,19})\n\z'){throw 'Owner refusal metadata unknown; retain stage'}
    $expectedBytes=[long]$Matches[1];$expectedSha=$Matches[2]
    if($expectedBytes -gt 8192){throw 'Owner refusal exceeds fixed bound'}
    $path=Join-Path $Packet 'capture-owner-refusal.json'
    if(Test-Path -LiteralPath $path){throw 'Owner refusal saved path already exists'}
    $copyPath=New-CaptureHistoricalTransportPath
    $Record.capture_owner_refusal_transport_path=$copyPath
    $copied=& $Copy ($Root+'/capture-owner-refusal.json') $copyPath
    if($copied.timeout -or $copied.exit -ne 0 -or -not(Test-Path -LiteralPath $copyPath -PathType Leaf) -or (Get-Item -LiteralPath $copyPath).Length -ne $expectedBytes -or (Get-FileHash -LiteralPath $copyPath).Hash.ToLowerInvariant() -cne $expectedSha){throw 'Owner refusal copy unknown; retain stage'}
    $returnedBytes=[IO.File]::ReadAllBytes($copyPath)
    if($returnedBytes.Length -ne $expectedBytes -or [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($returnedBytes)).ToLowerInvariant() -cne $expectedSha){throw 'Owner refusal returned bytes changed; retain stage'}
    $saved=[IO.FileStream]::new($path,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try{$saved.Write($returnedBytes,0,$returnedBytes.Length);$saved.Flush($true)}finally{$saved.Dispose()}
    if((Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -cne $expectedSha){throw 'Owner refusal saved bytes unknown; retain stage'}
    # Complete returned bytes remain preservation knowledge after later refusal.
    $Record.capture_owner_refusal_saved_copy_verified=$true
    $Record.capture_owner_refusal_saved_copy_sha256=$expectedSha
    $Record.capture_owner_refusal_state='preserved-unbound'
    $Record.capture_owner_refusal_decoded=$false
    $after=& $Read $command
    if($after.timeout -or $after.exit -ne 0 -or $after.stdout -cne $before.stdout){throw 'Owner refusal later binding unknown; preserve copy and retain stage'}
    $Record.capture_owner_refusal_state='preserved'
    $value=$null
    try{$raw=[Text.UTF8Encoding]::new($false,$true).GetString([IO.File]::ReadAllBytes($path));$value=ConvertFrom-CaptureOwnerRefusalRaw $raw}catch{}
    $Record.capture_owner_refusal_decoded=Test-CaptureOwnerRefusal $value $Nonce $Identity.attempt_pid $Identity.attempt_start $Identity.root_device $Identity.root_inode $FocusAncestry $ReceiverSubtreeCapture $ReceiverSubtreeCapture512 $ReceiverSubtreeItemCap
    if($Record.capture_owner_refusal_decoded){$Record.capture_owner_refusal=$value}
}
