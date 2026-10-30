# Restored historical preservation only; this never grants capture/facts admission.
. "$PSScriptRoot/capture-historical-transport.ps1"
function Receive-CaptureRequestHistory([string]$Root,[string]$Nonce,$Identity,[string]$Packet,[Collections.IDictionary]$Record,[scriptblock]$Read,[scriptblock]$Copy) {
    if($Nonce -cnotmatch '\A[0-9a-f]{32}\z' -or $Root -cne ('/run/rmb-qt-probe-'+$Nonce)){throw 'Historical request root refused'}
    foreach($name in @('attempt_pid','attempt_start','root_device','root_inode')){
        $parsed=[ulong]0
        if($Identity.$name -isnot [string] -or $Identity.$name -cnotmatch '\A[1-9][0-9]{0,19}\z' -or -not [ulong]::TryParse($Identity.$name,[ref]$parsed)){throw 'Historical original identity refused'}
    }
    $canonical=$Nonce+' '+$Identity.attempt_pid+' '+$Identity.attempt_start+' '+$Identity.root_device+' '+$Identity.root_inode+" capture-observation 120000 main-dev-facts-120s`n"
    $canonicalBytes=[Text.Encoding]::UTF8.GetBytes($canonical)
    $canonicalSha=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($canonicalBytes)).ToLowerInvariant()
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
if test -d '/proc/@PID@'; then
 current_start=$(awk '{print $22}' '/proc/@PID@/stat')
 case "$current_start" in ''|*[!0-9]*) exit 90;; esac
 test "$current_start" != '@START@'
else
 test ! -e '/proc/@PID@' && test ! -L '/proc/@PID@'
fi
'@
    $guard=$guard.Replace('@ROOT@',$Root).Replace('@NONCE@',$Nonce).Replace('@DEV@',$Identity.root_device).Replace('@INO@',$Identity.root_inode).Replace('@PID@',$Identity.attempt_pid).Replace('@START@',$Identity.attempt_start)
    foreach($item in @(
        @{name='capture-observation-request';prefix='capture_request'},
        @{name='capture-observation-request.tmp';prefix='capture_request_tmp'})){
        $Record[$item.prefix+'_history_binding_verified']=$false
        $metadata=@'
if test ! -e "$root/@NAME@" && test ! -L "$root/@NAME@"; then
 printf 'absent\n'
else
 test -f "$root/@NAME@" && test ! -L "$root/@NAME@"
 test "$(stat -c '%a %u' "$root/@NAME@")" = '600 0'
 bytes=$(wc -c < "$root/@NAME@"); test "$bytes" -le 256
 hash=$(sha256sum "$root/@NAME@" | awk '{print $1}')
 printf 'present %s %s ' "$bytes" "$hash"
 stat -c '%d %i' "$root/@NAME@"
fi
'@
        $command=$guard+"`n"+$metadata.Replace('@NAME@',$item.name)+"`n"+$guard
        $before=& $Read $command
        if($before.timeout -or $before.exit -ne 0){throw 'Historical request metadata unknown; retain stage'}
        if($before.stdout -ceq "absent`n"){$Record[$item.prefix+'_history_state']='absent';continue}
        if($before.stdout -cnotmatch '\Apresent (0|[1-9][0-9]{0,2}) ([0-9a-f]{64}) ([1-9][0-9]{0,19}) ([1-9][0-9]{0,19})\n\z'){throw 'Historical request metadata refused; retain stage'}
        $expectedBytes=[long]$Matches[1];$expectedSha=$Matches[2]
        if($expectedBytes -gt 256){throw 'Historical request exceeds fixed bound'}
        $path=Join-Path $Packet $item.name
        $flag=$item.prefix+'_saved_copy_verified';$shaFlag=$item.prefix+'_saved_copy_sha256'
        if(Test-Path -LiteralPath $path){
            if(-not $Record[$flag] -or $Record[$shaFlag] -cne $expectedSha -or $expectedSha -cne $canonicalSha -or -not(Test-Path -LiteralPath $path -PathType Leaf) -or (Get-Item -LiteralPath $path).Length -ne $canonicalBytes.Length -or (Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -cne $canonicalSha){throw 'Historical request local path cannot be overwritten'}
        }else{
            $copyPath=New-CaptureHistoricalTransportPath
            $Record[$item.prefix+'_history_transport_path']=$copyPath
            $copied=& $Copy ($Root+'/'+$item.name) $copyPath
            if($copied.timeout -or $copied.exit -ne 0 -or -not(Test-Path -LiteralPath $copyPath -PathType Leaf) -or (Get-Item -LiteralPath $copyPath).Length -ne $expectedBytes -or (Get-FileHash -LiteralPath $copyPath).Hash.ToLowerInvariant() -cne $expectedSha){throw 'Historical request copy unknown; retain stage'}
            $bytes=[IO.File]::ReadAllBytes($copyPath)
            if($bytes.Length -ne $expectedBytes -or [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant() -cne $expectedSha){throw 'Historical request returned bytes changed'}
            $saved=[IO.FileStream]::new($path,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
            try{$saved.Write($bytes,0,$bytes.Length);$saved.Flush($true)}finally{$saved.Dispose()}
            if((Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -cne $expectedSha){throw 'Historical request saved bytes unknown'}
            $Record[$item.prefix+'_history_copy_verified']=$true
            $Record[$item.prefix+'_history_copy_sha256']=$expectedSha
        }
        $Record[$item.prefix+'_history_state']='preserved-unbound'
        if($expectedSha -cne $canonicalSha -or $expectedBytes -ne $canonicalBytes.Length){throw 'Historical request bytes are noncanonical; preserve copy and retain stage'}
        $Record[$flag]=$true;$Record[$shaFlag]=$expectedSha
        $after=& $Read $command
        if($after.timeout -or $after.exit -ne 0 -or $after.stdout -cne $before.stdout){throw 'Historical request later binding unknown; preserve copy and retain stage'}
        $Record[$item.prefix+'_history_state']='preserved'
        $Record[$item.prefix+'_history_binding_verified']=$true
    }
}
