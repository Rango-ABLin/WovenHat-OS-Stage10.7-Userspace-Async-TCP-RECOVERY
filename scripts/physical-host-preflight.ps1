# Read-only host inventory. Never changes boot entries, drivers or disks.
param([string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $projectRoot ('audit-artifacts/physical-host-' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
}
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
$report = [ordered]@{
    TimeUtc = (Get-Date).ToUniversalTime().ToString('o')
    Elevated = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    Computer = Get-CimInstance Win32_ComputerSystem | Select-Object Manufacturer,Model,HypervisorPresent
    OS = Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version
    Adapters = @(Get-CimInstance Win32_NetworkAdapter | Where-Object { $_.PNPDeviceID -like 'PCI*' } | Select-Object Name,PNPDeviceID,NetEnabled)
    Disks = @(Get-Disk | Select-Object Number,FriendlyName,BusType,PartitionStyle,IsBoot,IsSystem,Size)
    SerialPorts = @(Get-CimInstance Win32_SerialPort | Select-Object DeviceID,Name)
    SecureBoot = $null
    AssignableDevices = $null
    BootConfiguration = $null
    PhysicalTestPerformed = $false
}
try { $report.SecureBoot = @{Known=$true; Enabled=(Confirm-SecureBootUEFI)} }
catch { $report.SecureBoot = @{Known=$false; Error=$_.Exception.Message} }
try { $report.AssignableDevices = @{Known=$true; Devices=@(Get-VMHostAssignableDevice | Select-Object InstanceID,LocationPath)} }
catch { $report.AssignableDevices = @{Known=$false; Error=$_.Exception.Message} }
$bootOutput = & bcdedit /enum firmware 2>&1
$report.BootConfiguration = @{ExitCode=$LASTEXITCODE; Output=($bootOutput -join "`n")}
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'host.json') -Encoding UTF8
Write-Output "Host inventory recorded: $OutputDirectory"
Write-Output 'PhysicalTestPerformed=False; this report is not native-driver acceptance.'
