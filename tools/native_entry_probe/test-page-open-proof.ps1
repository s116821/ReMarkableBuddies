$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'page-open-proof.ps1')
$nonce = '0123456789abcdef0123456789abcdef'
function New-Callback {
    [pscustomobject]@{nonce=$nonce;stage='open-observed';application_thread=$true;engine_thread=$true;helper_available=$true;controller_available=$true}
}
$cases = 0
function Require-Result($callback, [bool]$expected, [string]$name, [string]$expectedNonce=$nonce) {
    $script:cases++
    $actual = Test-PageOpenCallback $callback $expectedNonce
    if ($actual -isnot [bool] -or $actual -ne $expected) { throw ('Proof case failed: '+$name) }
}
Require-Result (New-Callback) $true 'strict live shape'
Require-Result $null $false 'absent callback'
foreach ($stage in @('resolved','deadline','open-cancelled','open-refused','open-observed ','OPEN-OBSERVED','')) {
    $callback=New-Callback; $callback.stage=$stage
    Require-Result $callback $false ('stage '+$stage)
}
foreach ($field in @('application_thread','engine_thread','helper_available','controller_available')) {
    foreach ($value in @($false,'true',1,$null)) {
        $callback=New-Callback; $callback.$field=$value
        Require-Result $callback $false ('typed field '+$field)
    }
    $callback=New-Callback; $callback.PSObject.Properties.Remove($field)
    Require-Result $callback $false ('missing field '+$field)
}
foreach ($field in @('nonce','stage')) {
    $callback=New-Callback; $callback.PSObject.Properties.Remove($field)
    Require-Result $callback $false ('missing '+$field)
    $callback=New-Callback; $callback.$field=123
    Require-Result $callback $false ('nonstring '+$field)
}
foreach ($value in @('fedcba9876543210fedcba9876543210','0123456789ABCDEF0123456789ABCDEF','0123456789abcdef0123456789abcdef ')) {
    $callback=New-Callback; $callback.nonce=$value
    Require-Result $callback $false 'nonce mismatch'
}
Require-Result (New-Callback) $false 'invalid expected nonce' 'short'
Require-Result ((New-Callback | ConvertTo-Json -Compress) | ConvertFrom-Json) $true 'JSON typed round trip'
Write-Output ('PASS '+$cases+' fixed open callback proof cases; no transport or device action')
