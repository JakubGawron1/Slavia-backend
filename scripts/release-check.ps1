param()
$ErrorActionPreference = 'Stop'

Write-Host "== Slavia-backend release check =="

Write-Host "[1/2] cargo check"
cargo check

Write-Host "[2/2] cargo test --lib"
cargo test --lib

Write-Host "OK: backend release check completed."
