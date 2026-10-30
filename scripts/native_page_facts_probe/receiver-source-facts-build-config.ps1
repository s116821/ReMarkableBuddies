. "$PSScriptRoot/capture-observation-build-config.ps1"
# Separate read-only probe, existing v11 prerequisites and exact six-page fixture.
function Get-ReceiverSourceFactsBuildConfig($Expected) {
    $config=Get-CaptureObservationBuildConfig $Expected $false $true $false 4096 32 $true $true $true $true
    return $config.Replace('    return config;',"    config.developmentReceiverSourceFacts=true;`n    return config;")
}
