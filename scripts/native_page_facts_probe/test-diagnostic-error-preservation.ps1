$ErrorActionPreference='Stop'
$source=Join-Path $PSScriptRoot 'run-qt-page-facts-diagnostic-source.ps1'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($source,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Operator syntax refused'}
$outer=$ast.Find({param($n) $n -is [Management.Automation.Language.TryStatementAst] -and $n.Body.Extent.Text.Contains('Require (SSH (Expand')},$true)
if(-not $outer){throw 'Actual operator try seam absent'}
$restoration=$outer.Finally.Find({param($n) $n -is [Management.Automation.Language.TryStatementAst]},$true)
# Replace transport/action bodies only. Exercise actual catches, postconditions and receipt finally.
$checks=$restoration.Body.Extent.Text.Substring($restoration.Body.Extent.Text.IndexOf('    if(-not $record.cleanup_verified)'))
$checks=$checks.Substring(0,$checks.LastIndexOf('}'))
$code=$outer.Extent.Text.Replace($outer.Body.Extent.Text,'{ & $PrimaryAction }').Replace($restoration.Body.Extent.Text,('{ & $RestorationAction;'+"`n"+$checks+'}'))
$temp=Join-Path ([IO.Path]::GetTempPath()) ('rem25-errors-local-'+[guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($temp)
$child=Join-Path $temp 'local-child.ps1'
[IO.File]::WriteAllText($child,$code,[Text.UTF8Encoding]::new($false))
$packet=$temp;$ReceiverSourceFacts=$true;$count=0
try{
 foreach($case in @('primary-and-cleanup','primary-only','restoration-only','success')){
  $record=[ordered]@{restored=$false;cleanup_verified=$false;receiver_source_facts_observed=$true}
  $PrimaryAction={if($case -like 'primary*'){throw 'local initiating error'}}
  $RestorationAction={if($case -ceq 'restoration-only'){throw 'local restoration error'};$record.restored=$true;$record.cleanup_verified=$case -cne 'primary-and-cleanup'}
  $raised=$null
  try{& $child|Out-Null}catch{$raised=$_}
  $saved=Get-Content -LiteralPath (Join-Path $temp 'operator-receipt.json') -Raw|ConvertFrom-Json
  if($case -like 'primary*'){
   if($saved.primary_error.message -cne 'local initiating error' -or $saved.primary_error.type -cne 'System.Management.Automation.RuntimeException' -or -not $saved.primary_error.script_stack.Contains('local-child.ps1')){throw 'Primary source error lost'}
  }elseif($saved.PSObject.Properties['primary_error']){throw 'Spurious primary error'}
  if($case -ceq 'primary-and-cleanup'){
   if(-not $raised -or $saved.restoration_error.message -notlike 'Restoration/stage uncertain*' -or $saved.cleanup_verified){throw 'Cleanup failure lost or promoted'}
  }elseif($case -ceq 'restoration-only'){
   if(-not $raised -or $saved.restoration_error.message -cne 'local restoration error'){throw 'Restoration exception lost'}
  }elseif($case -ceq 'primary-only'){
   if(-not $raised -or $raised.Exception.Message -cne 'local initiating error' -or $saved.PSObject.Properties['restoration_error']){throw 'Original propagation changed'}
  }elseif($raised -or -not $saved.cleanup_verified -or $saved.PSObject.Properties['restoration_error']){throw 'Successful finalization changed'}
  $count++
 }
 "PASS actual operator cross-script catches/finally $count cases; local actions only"
}finally{
 Remove-Item -LiteralPath $child -ErrorAction SilentlyContinue
 Remove-Item -LiteralPath (Join-Path $temp 'operator-receipt.json') -ErrorAction SilentlyContinue
 Remove-Item -LiteralPath $temp
}
