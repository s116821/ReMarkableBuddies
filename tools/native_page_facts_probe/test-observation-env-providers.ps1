$ErrorActionPreference='Stop'
$source=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'run-qt-input-observation-source.ps1')).Replace("`r",'')
$blocks=@([regex]::Matches($source,'(?m)^env_records=.*\n[^\n]+'))
if($blocks.Count -ne 3){throw 'Expected three exact environment guards'}
$guards=@($blocks|ForEach-Object {$_.Value.Replace('/proc/@STOCKPID@/environ','/fixture/environ').Replace('"/proc/$p/environ"','/fixture/environ').Replace('@ROOT@','/fixture')})
# Only acquisition tools are fault-injected; predicates are extracted unchanged.
$header=@'
set -eu
cat() {
 if test "$fixture_mode" = cat-missing; then return 127; fi
 if test "$fixture_mode" = missing; then /bin/cat /missing-env-fixture; return $?; fi
 if test "$fixture_mode" = eio; then /bin/cat /proc/self/mem; return $?; fi
 printf '%b' "$fixture_data"
 if test "$fixture_mode" = partial; then return 7; fi
}
tr() {
 if test "$fixture_mode" = tr-missing; then return 127; fi
 if test "$fixture_mode" = tr1 && test "$1" = '\n'; then return 7; fi
 if test "$fixture_mode" = tr2 && test "$1" = '\000'; then return 7; fi
 /usr/bin/tr "$@"
}
'@
$shell=@'
set -eu
count=0
run_case() {
 fixture_data=$1; fixture_mode=$2; expected=$3; guard=$4
 export fixture_data fixture_mode
 body="$header
$guard
printf 'CONTINUED\n'"
 if test "$fixture_mode" = unsupported; then body=$(printf '%s' "$body" | sed 's/set -o pipefail/set -o not-supported/'); fi
 set +e
 output=$($TEST_SHELL -c "$body" 2>/dev/null); status=$?
 set -e
 case "$expected" in
  pass) test "$status" = 0 && test "$output" = CONTINUED || { printf 'FAIL pass %s %s\n' "$fixture_mode" "$fixture_data"; exit 1; };;
  present) test "$status" = 90 && test "$output" != CONTINUED || { printf 'FAIL present\n'; exit 1; };;
  refuse) test "$status" != 0 && test "$output" != CONTINUED || { printf 'FAIL refusal %s\n' "$fixture_mode"; exit 1; };;
 esac
 count=$((count+1))
}
for key in QT_QPA_EVDEV_DEBUG QT_LOGGING_RULES QT_MESSAGE_PATTERN LD_PRELOAD LD_LIBRARY_PATH XOVI_ROOT; do
 case "$key" in QT*) guard=$qt_guard;; *) guard=$loader_guard;; esac
 for value in '' 0; do
  run_case "$key=$value\000OTHER=ok\000LAST=ok\000" normal present "$guard"
  run_case "OTHER=ok\000$key=$value\000LAST=ok\000" normal present "$guard"
  run_case "OTHER=ok\000LAST=ok\000$key=$value\000" normal present "$guard"
 done
done
for guard in "$qt_guard" "$loader_guard"; do
 run_case 'OTHER=ok\000LAST=ok\000' normal pass "$guard"
 run_case '' normal pass "$guard"
 run_case 'OTHER=value\nLD_PRELOAD=/fixture/payload.so\000' normal pass "$guard"
done
for data in 'LD_PRELOAD=/fixture/payload.so\000OTHER=ok\000' 'QT_QPA_EVDEV_DEBUG=1\000LD_PRELOAD=/fixture/payload.so\000QT_LOGGING_RULES=enabled\000' 'QT_QPA_EVDEV_DEBUG=1\000QT_LOGGING_RULES=enabled\000LD_PRELOAD=/fixture/payload.so\000' 'OTHER=value\nLD_PRELOAD=forged\000LD_PRELOAD=/fixture/payload.so\000'; do
 run_case "$data" normal pass "$ld_guard"
done
for data in '' 'OTHER=ok\000' 'LD_PRELOAD=/wrong\000' 'XLD_PRELOAD=/fixture/payload.so\000' 'LD_PRELOAD=/fixture/payload.so.more\000' 'OTHER=value\nLD_PRELOAD=/fixture/payload.so\000' 'LD_PRELOAD=/fixture/payload.so\n\000' 'LD_PRELOAD=/fixture/payload.so\r\000'; do
 run_case "$data" normal refuse "$ld_guard"
done
for guard in "$qt_guard" "$loader_guard" "$ld_guard"; do
 for mode in missing eio partial tr1 tr2 tr-missing cat-missing unsupported; do
  run_case 'LD_PRELOAD=/fixture/payload.so\000' "$mode" refuse "$guard"
 done
 run_case '' normal refuse "awk() { return 127; }
$guard"
done
printf 'PASS %s exact-provider environment cases\n' "$count"
'@
$quotedHeader=$header.Replace("'","'\''")
$prelude="header='$quotedHeader'`n"
foreach($pair in @(@('qt_guard',$guards[0]),@('loader_guard',$guards[1]),@('ld_guard',$guards[2]))){$prelude+=$pair[0]+"='"+$pair[1].Replace("'","'\''")+"'`n"}
foreach($provider in @(@('sha256:416c7a7be0038156797b0892f031f352b841d1921fae83f712d0a272e4724618','/bin/bash','/usr/bin/tr'),@('busybox@sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662','/bin/sh','/bin/tr'))){
    $fixture=($prelude+$shell).Replace('/usr/bin/tr',$provider[2])
    $output=$fixture | docker run --rm -i --network none --read-only --env "TEST_SHELL=$($provider[1])" --entrypoint /bin/sh $provider[0] -c 'tr -d "\r" | "$TEST_SHELL"'
    if($LASTEXITCODE -ne 0){throw "Provider fixture failed: $($provider[0])"}
    Write-Output "$($provider[0]) $output"
}
# Vendor /bin/sh is dash: exact required pipefail refuses before acquisition.
$unsupported="set -eu`n"+$guards[0]+"`nprintf 'CONTINUED\n'"
$output=$unsupported | docker run --rm -i --network none --read-only --entrypoint /bin/sh sha256:416c7a7be0038156797b0892f031f352b841d1921fae83f712d0a272e4724618 -c 'tr -d "\r" | /bin/sh'
if($LASTEXITCODE -eq 0 -or $output -contains 'CONTINUED'){throw 'Unsupported actual shell pipefail continued'}
Write-Output 'PASS exact unsupported dash pipefail refusal'
