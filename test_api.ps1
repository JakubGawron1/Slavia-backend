$ErrorActionPreference = "Stop"

Write-Output "Logowanie jako superadmin..."
$loginBody = '{"username":"superadmin", "password":"superadmin123"}'
try {
    $response = Invoke-RestMethod -Uri "http://127.0.0.1:8000/api/auth/login" -Method Post -Headers @{"Content-Type"="application/json"} -Body $loginBody
    $token = $response.token
    Write-Output "Zalogowano! Token: $($token.Substring(0, 15))..."
} catch {
    Write-Output "Błąd logowania: $($_.Exception.Message)"
    if ($_.Exception.Response) {
        $reader = New-Object System.IO.StreamReader($_.Exception.Response.GetResponseStream())
        $reader.ReadToEnd()
    }
    exit
}

$headers = @{"Authorization"="Bearer $token"; "Content-Type"="application/json"}

Write-Output "`n[1] Dodawanie nowego zawodnika..."
$athleteBody = '{"username":"zawodnik1", "password":"zawodnikpassword", "first_name":"Jan", "last_name":"Kowalski"}'
try {
    $athResponse = Invoke-RestMethod -Uri "http://127.0.0.1:8000/api/athletes" -Method Post -Headers $headers -Body $athleteBody
    Write-Output "Dodano zawodnika! ID: $($athResponse.id)"
} catch {
    Write-Output "Błąd: $($_.Exception.Message)"
    if ($_.Exception.Response) {
        $reader = New-Object System.IO.StreamReader($_.Exception.Response.GetResponseStream())
        $reader.ReadToEnd()
    }
}

Write-Output "`n[2] Lista zawodników..."
Invoke-RestMethod -Uri "http://127.0.0.1:8000/api/athletes" -Method Get -Headers $headers | ConvertTo-Json

Write-Output "`n[3] Dodawanie nowych zawodów..."
$compBody = '{"title":"Zawody o Puchar", "date":"2026-05-10", "location":"Ruda Śląska", "description":"Ważne zawody"}'
Invoke-RestMethod -Uri "http://127.0.0.1:8000/api/competitions" -Method Post -Headers $headers -Body $compBody | ConvertTo-Json

Write-Output "`n[4] Lista zawodów..."
Invoke-RestMethod -Uri "http://127.0.0.1:8000/api/competitions" -Method Get -Headers $headers | ConvertTo-Json
