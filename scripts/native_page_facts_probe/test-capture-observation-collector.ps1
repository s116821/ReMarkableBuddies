$ErrorActionPreference='Stop'
. "$PSScriptRoot/capture-observation-proof.ps1"
. "$PSScriptRoot/capture-observation-collector.ps1"
$nonce='0123456789abcdef0123456789abcdef';$document='11111111-1111-1111-1111-111111111111';$page='22222222-2222-2222-2222-222222222222'
$root='/run/rmb-qt-probe-'+$nonce;$payload='a'*64
$png=[Convert]::FromBase64String('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a9d8AAAAASUVORK5CYII=')
$sha=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($png)).ToLowerInvariant()
$tokens=$null;$errors=$null;$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'test-capture-observation-proof.ps1'),[ref]$tokens,[ref]$errors)
$fixture=$ast.Find({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'New-Completion'},$true)
. ([scriptblock]::Create($fixture.Extent.Text))
$count=0
function Check([bool]$ok,[string]$name){if(-not $ok){throw "FAIL $name"};$script:count++}
$base=Join-Path ([IO.Path]::GetTempPath()) ('capture-collector-'+[Guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $base)
try{
    foreach($case in @('good','not-ready','status-timeout','initial-loss','completion-malformed','completion-duplicate','completion-escaped-duplicate','request-mismatch','copy-timeout','image-mismatch','postcopy-loss','completion-replaced','final-loss')){
        $packet=Join-Path $base $case;[void](New-Item -ItemType Directory -Path $packet)
        $record=[ordered]@{capture_verified=$false};$identityReads=0;$completionReads=0
        $completion=New-Completion;$json=$completion|ConvertTo-Json -Compress
        if($case -ceq 'completion-duplicate'){$json=$json.Insert(1,'"version":1,')}
        if($case -ceq 'completion-escaped-duplicate'){$json=$json.Insert(1,'"vers\u0069on":1,')}
        $read={param($command)
            if($command.Contains('capture-observation-complete.json')){
                $script:completionReads++
                if($case -ceq 'not-ready'){return @{exit=3;timeout=$false;stdout=''}}
                if($case -ceq 'status-timeout'){return @{exit=1;timeout=$true;stdout='partial'}}
                if($case -ceq 'completion-malformed'){return @{exit=0;timeout=$false;stdout='{}'}}
                if($case -ceq 'completion-replaced' -and $completionReads -gt 1){return @{exit=0;timeout=$false;stdout='{}'}}
                return @{exit=0;timeout=$false;stdout=$json}
            }
            if($command.Contains('capture-observation-request')){
                return @{exit=0;timeout=$false;stdout=$(if($case -ceq 'request-mismatch'){'wrong'}else{$nonce+" 1234 5678 11 22 capture-observation 120000 main-dev-facts-120s`n"})}
            }
            $script:identityReads++
            $lost=$case -ceq 'initial-loss' -or ($case -ceq 'postcopy-loss' -and $identityReads -ge 2) -or ($case -ceq 'final-loss' -and $identityReads -ge 3)
            return @{exit=0;timeout=$false;stdout=$(if($lost){"1234 9999 11 22`n"}else{"1234 5678 11 22`n"})}
        }
        $copy={param($remote,$local)
            if($case -ceq 'copy-timeout'){return @{exit=1;timeout=$true;stdout=''}}
            [IO.File]::WriteAllBytes($local,$(if($case -ceq 'image-mismatch'){[byte[]]@(0)}else{$png}));return @{exit=0;timeout=$false;stdout=''}
        }
        $failed=$false;$result=$false
        try{$result=Receive-CaptureObservation $root $nonce $payload ([pscustomobject]@{document=$document;order=@($page)}) $packet $record $read $copy}catch{$failed=$true}
        Check ($failed -eq ($case -cnotin @('good','not-ready'))) "$case failure"
        Check ($result -eq ($case -ceq 'good') -and $record.capture_verified -eq ($case -ceq 'good')) "$case admission"
        if($case -cin @('completion-duplicate','completion-escaped-duplicate')){Check (@(Get-ChildItem -LiteralPath $packet).Count -eq 0 -and -not $record.capture_completion_saved_copy_verified) "$case unknown outputs retained"}
        if($case -cin @('good','postcopy-loss','completion-replaced','final-loss')){Check ($record.capture_png_saved_copy_verified -and $record.capture_completion_saved_copy_verified -and $record.capture_request_saved_copy_verified) "$case preserved knowledge"}
        if($case -cin @('copy-timeout','image-mismatch')){Check ($record.capture_completion_saved_copy_verified -and -not $record.capture_png_saved_copy_verified) "$case partial evidence"}
        if($case -ceq 'good'){
            foreach($positive in @($true,$false)){
                $review=New-CaptureVisualReview $completion $record.capture_completion_saved_copy_sha256 $record.capture_request_saved_copy_sha256 $positive
                Check (@($review.PSObject.Properties).Count -eq 15 -and $review.visual_open_fixture -eq $positive -and $review.reviewer -ceq 'Main') 'explicit visual decision'
            }
            $metadata={param($command)
                if($command.Contains('capture-owner-refusal.json') -or $command.Contains('capture-visual-review.json') -or $command.Contains('capture-observation-request.tmp') -or $command.Contains('publish-captured-facts-source.sh')){return @{exit=0;timeout=$false;stdout="absent`n"}}
                $name=if($command.Contains('capture-window.png')){'capture-window.png'}elseif($command.Contains('capture-observation-complete.json')){'capture-observation-complete.json'}else{'capture-observation-request'}
                $path=Join-Path $packet $name;$hash=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant();$bytes=(Get-Item -LiteralPath $path).Length
                return @{exit=0;timeout=$false;stdout="present $bytes $hash`n"}
            }
            Check (Test-CapturePreserved $root $packet $record $metadata) 'all known output copies preserved'
            $record.capture_png_saved_copy_verified=$false;Check (-not(Test-CapturePreserved $root $packet $record $metadata)) 'unverified output retained'
            Check (-not(Test-CapturePreserved $root $packet $record {param($command)@{exit=1;timeout=$true;stdout=''}})) 'unknown transport retained'
            Check (Test-CapturePreserved $root $packet $record {param($command)@{exit=0;timeout=$false;stdout="absent`n"}}) 'positive output absence'
        }
    }
    Write-Output "PASS capture collector: $count checks (mocked transport only)"
}finally{
    $resolved=[IO.Path]::GetFullPath($base);$prefix=[IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if(-not $resolved.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -cnotmatch '\Acapture-collector-[0-9a-f]{32}\z'){throw 'Fixture cleanup path refused'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
