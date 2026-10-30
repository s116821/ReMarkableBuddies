param([Parameter(Mandatory=$true)][string]$ReadinessDirectory,[Parameter(Mandatory=$true)][string]$EvidenceDirectory)
$ErrorActionPreference='Stop'
. "$PSScriptRoot/readiness-dependency.ps1"
Assert-ReadFactsReadinessDependency $ReadinessDirectory
if(Test-Path -LiteralPath $EvidenceDirectory){throw 'Readiness fixture output already exists'}
[void](New-Item -ItemType Directory -Path $EvidenceDirectory)
$source=[IO.File]::ReadAllText((Join-Path $ReadinessDirectory 'wait-readiness.ps1'))
$observer=[IO.File]::ReadAllBytes((Join-Path $ReadinessDirectory 'observe-readiness.sh'))
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path (Split-Path $ReadinessDirectory) 'main-manual-actions.ps1'),[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Main manual source syntax refused'}
$expand=$ast.Find({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'Expand-ReadFactsAction'},$true)
if($null -eq $expand){throw 'Actual tuple parser absent'}
. ([scriptblock]::Create($expand.Extent.Text))
$nonce=[regex]::Match($expand.Extent.Text,'[0-9a-f]{32}').Value
$waiting="$nonce 1234 5678 11 22 waiting-facts 100 120000 main-dev-facts-120s"
$origin='a'*64;$count=0;$results=@()
function Check([bool]$ok,[string]$name){if(-not $ok){throw "FAIL $name"};$script:count++}
foreach($case in @('ready','pending-root','pending-waiting','two-pending','malformed-ready','malformed-pending','pending-stderr','wrong-origin','wrong-allowance','post-not-admitted','unknown-exit','timeout','no-receipt','observer-changed','stale-wrapper','replay-pass','expired-origin')){
    $directory=Join-Path $EvidenceDirectory $case;[void](New-Item -ItemType Directory -Path (Join-Path $directory 'readiness-source') -Force)
    $localReadiness=Join-Path $directory 'readiness-source'
    [IO.File]::WriteAllBytes((Join-Path $localReadiness 'observe-readiness.sh'),$observer)
    [IO.File]::WriteAllText((Join-Path $localReadiness 'wait-readiness.ps1'),$source,[Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllText((Join-Path $directory 'main-manual-actions.ps1'),'# Local transport mocks only.',[Text.UTF8Encoding]::new($false))
    if($case -ceq 'observer-changed'){[IO.File]::AppendAllText((Join-Path $localReadiness 'observe-readiness.sh'),"`n# changed`n")}
    if($case -ceq 'stale-wrapper'){
        $embedded=[regex]::Match($source,"-cne '([0-9a-f]{64})'\)\{throw 'Read-only observer changed'").Groups[1].Value
        [IO.File]::WriteAllText((Join-Path $localReadiness 'wait-readiness.ps1'),$source.Replace($embedded,('b'*64)),[Text.UTF8Encoding]::new($false))
    }
    $dependencyRefused=$false;try{Assert-ReadFactsReadinessDependency $localReadiness}catch{$dependencyRefused=$true}
    Check ($dependencyRefused -eq ($case -cin @('observer-changed','stale-wrapper'))) "prelaunch dependency $case"
    . (Join-Path $localReadiness 'wait-readiness.ps1')
    $calls=0;$admissions=0;$prefix=Join-Path $directory 'readiness';$originPath=Join-Path $directory 'immutable-origin'
    function Assert-ReadFactsHostAdmission([string]$Path,[string]$Sha,[int]$Allowance){
        Check ($Path -ceq $originPath -and $Sha -ceq $origin -and $Allowance -eq 5000) 'same original origin and allowance'
        $script:admissions++;if($case -ceq 'expired-origin'){throw 'Expired immutable origin'}
    }
    function Invoke-ReadFactsBounded([string]$Program,[string[]]$Arguments,[string]$Path,[string]$Sha,[string]$PassPrefix){
        $script:calls++
        Check ($Program -ceq 'C:/Windows/System32/OpenSSH/ssh.exe' -and $Arguments[-1] -ceq [IO.File]::ReadAllText((Join-Path $localReadiness 'observe-readiness.sh')) -and $Path -ceq $originPath -and $Sha -ceq $origin) 'actual observer bytes and fixed transport arguments'
        if($case -ceq 'ready' -or ($case -cin @('pending-root','pending-waiting') -and $calls -gt 1) -or ($case -ceq 'two-pending' -and $calls -gt 2)){return @{stdout=$waiting+"`n"}}
        if($case -ceq 'malformed-ready'){return @{stdout='foreign tuple'}}
        if($case -ceq 'no-receipt'){throw 'Mock transport unknown'}
        $receipt=[ordered]@{exit=[long]3;timeout=$false;post_admitted=$true;remote_exit_unknown=$false;origin_sha256=$origin;allowance_ms=[long]5000}
        $stdout=$(if($case -ceq 'pending-waiting'){"pending-waiting-absent`n"}else{"pending-root-absent`n"});$stderr=''
        switch($case){'malformed-pending'{$stdout="pending-root-absent`nextra"};'pending-stderr'{$stderr='noise'};'wrong-origin'{$receipt.origin_sha256='b'*64};'wrong-allowance'{$receipt.allowance_ms=[long]10000};'post-not-admitted'{$receipt.post_admitted=$false};'unknown-exit'{$receipt.remote_exit_unknown=$true};'timeout'{$receipt.timeout=$true}}
        [IO.File]::WriteAllText($PassPrefix+'.receipt.json',($receipt|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
        [IO.File]::WriteAllText($PassPrefix+'.stdout.txt',$stdout,[Text.UTF8Encoding]::new($false));[IO.File]::WriteAllText($PassPrefix+'.stderr.txt',$stderr,[Text.UTF8Encoding]::new($false))
        throw 'Mock pending or refused transport'
    }
    if($case -ceq 'replay-pass'){[IO.File]::WriteAllText($prefix+'-pass-000001.claim','existing')}
    $failed=$false;$returned=$null;try{$returned=Wait-ReadFactsReadiness $originPath $origin $prefix}catch{$failed=$true}
    $success=$case -cin @('ready','pending-root','pending-waiting','two-pending')
    Check ($failed -ne $success) "actual function outcome $case"
    if($success){Check ($returned -ceq $waiting -and $calls -eq $(if($case -ceq 'ready'){1}elseif($case -ceq 'two-pending'){3}else{2}) -and $admissions -eq $calls) "bounded pending to ready $case"}
    elseif($case -cin @('observer-changed','stale-wrapper')){Check ($calls -eq 0 -and $admissions -eq 0 -and -not(Test-Path -LiteralPath ($prefix+'.session.claim'))) "dependency refusal before any claim or transport $case"}
    elseif($case -cin @('expired-origin','replay-pass')){Check ($calls -eq 0) "refused before dispatch $case"}
    else{Check ($calls -eq 1) "malformed unknown never retried $case"}
    $results+=[ordered]@{case=$case;passed=$true;transport_calls=$calls;admission_calls=$admissions}
}
$receipt=[ordered]@{kind='actual-readiness-function-local-regression';source_directory=$ReadinessDirectory;observer_sha256=(Get-FileHash -LiteralPath (Join-Path $ReadinessDirectory 'observe-readiness.sh')).Hash.ToLowerInvariant();wrapper_sha256=(Get-FileHash -LiteralPath (Join-Path $ReadinessDirectory 'wait-readiness.ps1')).Hash.ToLowerInvariant();checks=$count;cases=$results;transport_mocked=$true;device_operations=$false;native_authority=$false}
[IO.File]::WriteAllText((Join-Path $EvidenceDirectory 'result.json'),($receipt|ConvertTo-Json -Depth 6),[Text.UTF8Encoding]::new($false))
Write-Output "PASS actual readiness function $count checks; all transports mocked"
