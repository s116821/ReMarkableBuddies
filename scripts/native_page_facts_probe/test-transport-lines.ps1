param([Parameter(Mandatory=$true)][string]$FixturePath)
$ErrorActionPreference='Stop'
$source=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-page-facts-diagnostic-source.ps1'))
# Evaluate the exact transport functions, mocking only Native. Never invoke SSH.
$begin=$source.IndexOf('function SSH(');$end=$source.IndexOf('function Require(', $begin)
. ([scriptblock]::Create($source.Substring($begin,$end-$begin)))
$count=0
function Check([bool]$ok,[string]$name){if(-not $ok){throw "FAIL $name"};$script:count++}
$script:calls=@();$script:advance=$false
function Native([string]$program,[string[]]$arguments,[int]$timeoutMs=20000){
 $script:calls+=@{program=$program;arguments=$arguments;timeout_ms=$timeoutMs}
 if($script:advance){$observationClock.ElapsedMilliseconds=150000}
 return @{timeout=$false;exit=0;stdout='owned'}
}
$budget=@{LiveObservationMs=150000};$observationClock=@{ElapsedMilliseconds=149000}
# A literal here-string parsed from a CRLF source buffer, as in the actual packet.
# The shell payload is harmless but contains quoting and literal dollar syntax.
$literal="@'`r`nset -eu`r`nprintf '%s\n' 'owned `$value'`r`n'@"
$raw=& ([scriptblock]::Create($literal))
Check ($raw.Contains("`r`n")) 'actual parsed here-string retains CRLF'
$lf=$raw.Replace("`r`n","`n")
$result=SSH $raw
Check ($calls.Count -eq 1 -and $calls[0].arguments[-1] -ceq $lf) 'ordinary transport receives LF exact payload'
Check ($calls[0].program -ceq 'ssh' -and $calls[0].timeout_ms -eq 20000 -and $calls[0].arguments[-2] -ceq 'RM2') 'ordinary transport options unchanged'
Check ($result.stdout -ceq 'owned') 'ordinary result unchanged'
$result=ObservationSSH $raw
Check ($calls.Count -eq 2 -and $calls[1].arguments[-1] -ceq $lf) 'observation transport receives LF exact payload'
Check ($calls[1].timeout_ms -eq 1000 -and $calls[1].arguments[7] -ceq 'ConnectTimeout=2') 'original remaining observation budget retained'
Check ($result.stdout -ceq 'owned') 'observation result unchanged'
[void](SSH $lf)
Check ($calls[2].arguments[-1] -ceq $lf) 'existing LF payload unchanged'
$observationClock.ElapsedMilliseconds=150000;$threw=$false
try{[void](ObservationSSH $raw)}catch{$threw=$true}
Check ($threw -and $calls.Count -eq 3) 'expired clock refuses before transport'
$observationClock.ElapsedMilliseconds=149999;$script:advance=$true;$threw=$false
try{[void](ObservationSSH $raw)}catch{$threw=$true}
Check ($threw -and $calls.Count -eq 4 -and $calls[3].timeout_ms -eq 1) 'late transport result refuses on original clock'
$fixture=@{raw=$raw;ssh=$calls[0].arguments[-1];observation=$calls[1].arguments[-1]}
[IO.File]::WriteAllText($FixturePath,($fixture|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
Write-Output "PASS $count exact transport CRLF/arguments/results/original-clock checks; Native mocked, no SSH"
