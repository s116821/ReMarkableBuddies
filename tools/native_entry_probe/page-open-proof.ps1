# Fixed proof for the explicitly selected development open/observe packet.
# Historical or post-restoration callbacks never establish live generation.
function Test-PageOpenCallback($Callback, [string]$ExpectedNonce) {
    if ($ExpectedNonce -cnotmatch '^[0-9a-f]{32}$') { return $false }
    return $null -ne $Callback -and
        $Callback.nonce -is [string] -and $Callback.nonce -ceq $ExpectedNonce -and
        $Callback.stage -is [string] -and $Callback.stage -ceq 'open-observed' -and
        $Callback.application_thread -is [bool] -and $Callback.application_thread -and
        $Callback.engine_thread -is [bool] -and $Callback.engine_thread -and
        $Callback.helper_available -is [bool] -and $Callback.helper_available -and
        $Callback.controller_available -is [bool] -and $Callback.controller_available
}
