$ErrorActionPreference='Stop'
# Host-only actual shell check of the exact source guard. No SSH or journal.
$source=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-input-observation-source.ps1'))
$guard=([regex]::Match($source,'(?m)^env_records=.*\r?\n.*QT_QPA_EVDEV_DEBUG.*\r?$')).Value.Replace("`r",'')
if(-not $guard){throw 'Missing exact environment guard'}
$cases=@(@{name='absent';input='OTHER=value';status=0})
foreach($key in @('QT_QPA_EVDEV_DEBUG','QT_LOGGING_RULES','QT_MESSAGE_PATTERN')){
    foreach($value in @('','0')){$cases+=@{name="$key=$value";input="$key=$value";status=90}}
}
$count=0
foreach($case in $cases){
    $command="printf '%s\000' '$($case.input)' | ("+$guard.Replace('/proc/@STOCKPID@/environ','/dev/stdin')+")"
    $fixture="set -e`n$command`nprintf 'ABSENT_OK\n'"
    $output=$fixture | docker run --rm -i --network none --read-only --entrypoint /bin/sh sha256:416c7a7be0038156797b0892f031f352b841d1921fae83f712d0a272e4724618 -c 'tr -d "\r" | bash'
    if($LASTEXITCODE -ne $case.status -or ($case.status -ne 0 -and $output -contains 'ABSENT_OK')){throw "Environment fixture failed: $($case.name)"}
    $count++
}
foreach($path in @('/missing-evdev-env-fixture','/proc/1/mem')){
    $fixture="set -e`n"+$guard.Replace('/proc/@STOCKPID@/environ',$path)+"`nprintf 'ABSENT_OK\n'"
    $output=$fixture | docker run --rm -i --network none --read-only --entrypoint /bin/sh sha256:416c7a7be0038156797b0892f031f352b841d1921fae83f712d0a272e4724618 -c 'tr -d "\r" | bash'
    if($LASTEXITCODE -eq 0 -or $output -contains 'ABSENT_OK'){throw "Environment read error passed: $path"}
    $count++
}
Write-Output "input-evdev environment shell: PASS $count cases (pinned local Docker; no device)"
