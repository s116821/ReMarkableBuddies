param([string]$HistoricalPath='', [string]$HistoricalSha256='')
$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-owner-refusal-proof.ps1"
. "$PSScriptRoot/capture-owner-refusal-collector.ps1"
$count=0;$nonce='0123456789abcdef0123456789abcdef'
function Check([bool]$ok,[string]$name){if(-not $ok){throw "FAIL $name"};$script:count++}
function New-V1 {
    [pscustomobject][ordered]@{kind='development-capture-owner-refusal';version=1;nonce=$nonce;attempt_pid='1234';attempt_start='5678';root_device='11';root_inode='22';setup_profile='main-dev-facts-120s';capture_accepted_ms=100;failure_ms=101;deadline_check_ms=$null;effective_deadline_ms=5100;branch='initial-progress';predicate='invalidated';discovery_result=$null;visited_items=$null;receiver_candidates=$null;scene_candidates=$null;matched_pairs=$null;first_pair_receiver=$null;first_pair_scene=$null;first_pair_rejection=$null;active_owner_rejection=$null;observer_role=$null;observer_member=$null;observer_failure=$null;native_authority=$false;render_authority=$false;ui_acknowledged=$false}
}
function New-V2([string]$site='none') {
    $v=New-V1;$v.version=2
    foreach($field in @('topology_limit','topology_depth','topology_queue_size','topology_child_count')){$v|Add-Member -NotePropertyName $field -NotePropertyValue $null}
    if($site -cne 'none'){
        $v.branch='owner-discovery';$v.predicate=$null;$v.discovery_result='open-topology-bound';$v.visited_items=2;$v.receiver_candidates=1;$v.scene_candidates=0;$v.topology_limit=$site
        switch($site){'visited'{$v.visited_items=4097};'depth'{$v.topology_depth=25};'queue-cap'{$v.topology_depth=1;$v.topology_queue_size=4096;$v.topology_child_count=1}}
    }
    return $v
}
function Valid($v){Test-CaptureOwnerRefusal $v $nonce '1234' '5678' '11' '22'}
function Digest([byte[]]$bytes){[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()}
foreach($site in @('none','visited','depth','queue-cap')){
    $v=New-V2 $site;Check (Valid $v) "valid tuple $site"
    Check (Valid (ConvertFrom-CaptureOwnerRefusalRaw ($v|ConvertTo-Json -Compress))) "raw tuple $site"
    foreach($field in @($v.PSObject.Properties.Name)){$bad=New-V2 $site;$bad.PSObject.Properties.Remove($field);Check (-not(Valid $bad)) "missing $site $field"}
}
$v=New-V1;Check (Valid $v) 'unchanged version1 exact29'
$v.branch='owner-discovery';$v.predicate=$null;$v.discovery_result='open-topology-bound';$v.visited_items=3657;$v.receiver_candidates=1;$v.scene_candidates=2
Check (Valid $v) 'version1 below visited cap leaves site unknown'
$v=New-V2 'queue-cap';$v.visited_items=1;$v.topology_depth=0;$v.topology_queue_size=1;$v.topology_child_count=4096;Check (Valid $v) 'queue minimum valid'
$v.topology_child_count=[long]::MaxValue;Check (Valid $v) 'child signed64 maximum is represented without clamp'
$v=New-V2 'queue-cap';$v.topology_depth=24;Check (Valid $v) 'queue depth24 boundary valid'
foreach($case in @('visited-low','visited-depth','visited-queue','visited-child','depth24','depth26','depth-overvisited','depth-queue','depth-child','queue-depth25','queue-zero','queue-over','queue-child-equal','queue-child-zero','queue-visited-overqueue','queue-visited-overcap','null-limit','unknown-limit','case-limit','other-branch','other-result','pair','active','predicate','deadline')){
    $v=New-V2 $(if($case.StartsWith('visited')){'visited'}elseif($case.StartsWith('depth')){'depth'}else{'queue-cap'})
    switch($case){
        'visited-low'{$v.visited_items=4096};'visited-depth'{$v.topology_depth=25};'visited-queue'{$v.topology_queue_size=1};'visited-child'{$v.topology_child_count=1}
        'depth24'{$v.topology_depth=24};'depth26'{$v.topology_depth=26};'depth-overvisited'{$v.visited_items=4097};'depth-queue'{$v.topology_queue_size=4096};'depth-child'{$v.topology_child_count=1}
        'queue-depth25'{$v.topology_depth=25};'queue-zero'{$v.topology_queue_size=0};'queue-over'{$v.topology_queue_size=4097};'queue-child-equal'{$v.topology_child_count=0};'queue-child-zero'{$v.topology_queue_size=1;$v.topology_child_count=0};'queue-visited-overqueue'{$v.topology_queue_size=1;$v.topology_child_count=4096};'queue-visited-overcap'{$v.visited_items=4097}
        'null-limit'{$v.topology_limit=$null};'unknown-limit'{$v.topology_limit='frontier'};'case-limit'{$v.topology_limit='Depth'};'other-branch'{$v.branch='initial-progress'};'other-result'{$v.discovery_result='open-item-lost'};'pair'{$v.matched_pairs=0};'active'{$v.active_owner_rejection='document-identity'};'predicate'{$v.predicate='deadline'};'deadline'{$v.deadline_check_ms=5100}
    }
    Check (-not(Valid $v)) "invalid matrix $case"
}
foreach($field in @('topology_depth','topology_queue_size','topology_child_count')){
    foreach($wrong in @($true,'1',1.5,-1,[bigint]::Parse('9223372036854775808'),$null)){$v=New-V2 'queue-cap';$v.$field=$wrong;Check (-not(Valid $v)) "scalar type $field $wrong"}
}
foreach($wrong in @(0,3,'2',2.5,$true)){$v=New-V2;$v.version=$wrong;Check (-not(Valid $v)) 'version finite integer'}
$v=New-V2;$v.version=1;Check (-not(Valid $v)) 'version1 with new fields rejected'
$v=New-V1;$v.version=2;Check (-not(Valid $v)) 'version2 old-only schema rejected'
foreach($branch in @('initial-progress','owner-discovery','observer-install','owner-revalidation')){
    $v=New-V2
    switch($branch){
        'owner-discovery'{$v.branch=$branch;$v.predicate=$null;$v.discovery_result='open-owner-unavailable';$v.visited_items=1;$v.receiver_candidates=0;$v.scene_candidates=0;$v.matched_pairs=0}
        'observer-install'{$v.branch=$branch;$v.predicate=$null;$v.discovery_result='open-owner-observed';$v.observer_role='document';$v.observer_member='pageMapChanged()';$v.observer_failure='signal-missing'}
        'owner-revalidation'{$v.branch=$branch;$v.predicate='invalidated-before';$v.discovery_result='open-owner-observed'}
    }
    Check (Valid $v) "non-topology v2 $branch"
    foreach($field in @('topology_limit','topology_depth','topology_queue_size','topology_child_count')){
        $bad=$v|ConvertTo-Json|ConvertFrom-Json;$bad.$field=$(if($field -ceq 'topology_limit'){'depth'}else{1});Check (-not(Valid $bad)) "non-topology requires null $branch $field"
    }
}
$raw=New-V2 'queue-cap'|ConvertTo-Json -Compress
foreach($key in @('version','topology_depth','topology_d\u0065pth')){Check ($null -eq (ConvertFrom-CaptureOwnerRefusalRaw $raw.Insert(1,('"'+$key+'":1,')))) 'duplicate raw key including escaped topology name'}
$v=New-V2;$v|Add-Member -NotePropertyName extra -NotePropertyValue 0;Check (-not(Valid $v)) 'extra field rejected'
$v=New-V2;$v.PSObject.Properties.Remove('topology_limit');$v|Add-Member -NotePropertyName Topology_limit -NotePropertyValue $null;Check (-not(Valid $v)) 'case-sensitive closed fields'
$base=Join-Path ([IO.Path]::GetTempPath()) ('topology-test-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $base)
$testTransportAllocator=${function:New-CaptureHistoricalTransportPath}
function New-CaptureHistoricalTransportPath { & $testTransportAllocator $base }
try{
    foreach($case in @('v1','visited','depth','queue-cap','unknown','mixed','duplicate','malformed','partial','invalid-utf8','foreign','later-loss')){
        $v=$(if($case -ceq 'v1'){New-V1}else{New-V2 $(if($case -cin @('visited','depth','queue-cap')){$case}else{'none'})})
        if($case -ceq 'unknown'){$v.version=3};if($case -ceq 'mixed'){$v.version=1};if($case -ceq 'foreign'){$v.nonce='ffffffffffffffffffffffffffffffff'}
        $text=$v|ConvertTo-Json -Compress
        if($case -ceq 'duplicate'){$text=$text.Insert(1,'"topology_d\u0065pth":null,')};if($case -ceq 'malformed'){$text='broken'};if($case -ceq 'partial'){$text=$text.Substring(0,40)}
        $bytes=$(if($case -ceq 'invalid-utf8'){[byte[]]@(0xc3,0x28)}else{[Text.Encoding]::UTF8.GetBytes($text)})
        $sha=Digest $bytes;$reads=0;$packet=Join-Path $base $case;[void](New-Item -ItemType Directory -Path $packet)
        $record=[ordered]@{capture_verified=$false;facts_verified=$false}
        $read={param($command)$script:reads++;Check ($command.Contains('11 22 700 0') -and $command.Contains('1234 5678')) 'actual original identity metadata guard';if($case -ceq 'later-loss' -and $script:reads -gt 1){return @{exit=1;timeout=$false;stdout=''}};@{exit=0;timeout=$false;stdout="present $($bytes.Length) $sha 11 99`n"}}
        $copy={param($remotePath,$localPath)Check ($record.capture_owner_refusal_transport_path -ceq $localPath) 'transport path recorded before preservation';[IO.File]::WriteAllBytes($localPath,$bytes);@{exit=0;timeout=$false}}
        $identity=[pscustomobject]@{attempt_pid='1234';attempt_start='5678';root_device='11';root_inode='22'}
        $refused=$false;try{Receive-CaptureOwnerRefusalEvidence ('/run/rmb-qt-probe-'+$nonce) $nonce $identity $packet $record $read $copy}catch{$refused=$true}
        Check ($refused -eq ($case -ceq 'later-loss')) "collection outcome $case"
        Check ($record.capture_owner_refusal_saved_copy_verified -and $record.capture_owner_refusal_saved_copy_sha256 -ceq $sha -and (Get-FileHash -LiteralPath (Join-Path $packet 'capture-owner-refusal.json')).Hash.ToLowerInvariant() -ceq $sha) "raw immutable byte knowledge $case"
        Check ([bool]$record.capture_owner_refusal_decoded -eq ($case -cin @('v1','visited','depth','queue-cap'))) "version/matrix decode $case"
        Check (-not $record.capture_verified -and -not $record.facts_verified) "no admission $case"
    }
    if($HistoricalPath){
        $before=[IO.File]::ReadAllBytes($HistoricalPath);Check ((Digest $before) -ceq $HistoricalSha256) 'historical exact supplied hash'
        $v=ConvertFrom-CaptureOwnerRefusalRaw ([Text.UTF8Encoding]::new($false,$true).GetString($before))
        Check ($v.version -eq 1 -and @($v.PSObject.Properties).Count -eq 29 -and (Test-CaptureOwnerRefusal $v $v.nonce $v.attempt_pid $v.attempt_start $v.root_device $v.root_inode)) 'historical version1 remains valid without site synthesis'
        Check ((Digest ([IO.File]::ReadAllBytes($HistoricalPath))) -ceq $HistoricalSha256) 'historical bytes unchanged after readback'
    }
    Write-Output "PASS topology version compatibility $count checks (actual decoder/collector; mocked transport)"
}finally{
    $resolved=[IO.Path]::GetFullPath($base)
    if(-not $resolved.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()),[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -cnotmatch '\Atopology-test-[0-9a-f]{32}\z'){throw 'Topology fixture cleanup path refused'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
