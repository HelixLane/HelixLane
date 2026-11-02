param([Parameter(Mandatory=$true)][string]$Executable)
$ErrorActionPreference = 'Stop'
$binary = (Resolve-Path -LiteralPath $Executable).Path
if (-not [System.IO.Path]::IsPathRooted($binary)) { throw 'Absolute executable path required' }
$root = 'HKCU:\Software\Classes\helixlane'
New-Item -Path "$root\shell\open\command" -Force | Out-Null
Set-Item -LiteralPath $root -Value 'URL:HelixLane connection'
New-ItemProperty -LiteralPath $root -Name 'URL Protocol' -Value '' -PropertyType String -Force | Out-Null
Set-Item -LiteralPath "$root\shell\open\command" -Value ('"' + $binary + '" "%1"')
