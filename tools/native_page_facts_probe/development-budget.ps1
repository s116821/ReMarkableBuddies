# Fixed opt-in DEV facts profile. No timer, transport or device effects here.
function Get-FactsDevelopmentBudget {
    return [pscustomobject]@{
        DevelopmentOptIn=$true
        SetupSelection='main-dev-facts-120s'
        SetupMs=120000
        ReadMs=5000
        LiveObservationMs=150000
        RollbackInitiationSeconds=180
        RestorationTimeoutStartSeconds=240
        SetupClockOrigin='entry-startup-monotonic'
        ReadClockOrigin='accepted-request-monotonic'
        LiveClockOrigin='host-observation-before-arming'
        RollbackClockOrigin='independent-timer-arming-before-activation'
    }
}
function Test-FactsLiveObservationWindow([long]$ElapsedMilliseconds) {
    return $ElapsedMilliseconds -ge 0 -and $ElapsedMilliseconds -lt 150000
}
