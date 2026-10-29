$ErrorActionPreference = 'SilentlyContinue'
$root = 'D:\_mroldriver\mrorCode'

$target = Join-Path $root 'target'
if (Test-Path $target) {
    $size = (Get-ChildItem $target -Recurse -File -Force | Measure-Object -Property Length -Sum).Sum
    Write-Output ("target exists: {0:N1} MB" -f ($size/1MB))
} else {
    Write-Output 'target not found'
}

Write-Output '--- cache dirs/files in project ---'
Get-ChildItem $root -Force -Directory | Where-Object { $_.Name -match '^(target|node_modules|\.cache|\.local|\.dev|\.next|dist|build|out)$' } | Select-Object -ExpandProperty Name
Get-ChildItem $root -Force -File | Where-Object { $_.Name -match '\.(log|tmp|lock)$|debug\.log' } | Select-Object Name, @{N='SizeKB';E={[math]::Round($_.Length/1KB,1)}}
