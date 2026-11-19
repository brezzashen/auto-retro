$ErrorActionPreference='SilentlyContinue'; $ProgressPreference='SilentlyContinue'
$smi="$env:SystemRoot\System32\nvidia-smi.exe"
$g=((& $smi --query-gpu=utilization.gpu,utilization.encoder,encoder.stats.sessionCount,encoder.stats.averageFps,encoder.stats.averageLatency,power.draw,temperature.gpu --format=csv,noheader,nounits) | Select-Object -First 1) -split ',\s*'
$r=[ordered]@{gpu_util=[int]$g[0]; enc_util=[int]$g[1]; enc_sessions=[int]$g[2]; enc_fps=[int]$g[3]; enc_latency_us=[int]$g[4]; power_w=[double]$g[5]; temp_c=[int]$g[6]; connected=$false}
$f="$env:ProgramFiles\Sunshine\config\sunshine.log"
if (Test-Path $f) {
  $fs=[IO.File]::Open($f,'Open','Read','ReadWrite'); $n=[Math]::Min($fs.Length,4194304); [void]$fs.Seek(-$n,'End'); $b=New-Object byte[] $n; [void]$fs.Read($b,0,$n); $fs.Close()
  $lines=[Text.Encoding]::UTF8.GetString($b) -split "`r?`n"
  $con=-1; $dis=-1; $ip=$null; $recent=0; $cut=(Get-Date).AddMinutes(-10)
  for ($i=0; $i -lt $lines.Count; $i++) { $l=$lines[$i]
    if ($l -match 'request from IP: ([0-9.]+),') { $ip=$Matches[1] }
    if ($l -match 'CLIENT CONNECTED') { $con=$i; if ($l -match '^\[([0-9-]+ [0-9:]+)') { if ([datetime]$Matches[1] -gt $cut) { $recent++ } } }
    if ($l -match 'CLIENT DISCONNECTED') { $dis=$i } }
  $r.client_ip=$ip; $r.connects_10m=$recent
  if ($con -gt $dis) {
    $r.connected=$true
    if ($lines[$con] -match '^\[([0-9-]+ [0-9:.]+)') { $r.connected_at=$Matches[1] }
    $re=0
    for ($i=[Math]::Max(0,$con-40); $i -lt $lines.Count; $i++) { $l=$lines[$i]
      if ($l -match 'clientViewport\): ([0-9]+x[0-9]+)') { $r.viewport=$Matches[1] }
      if ($i -gt $con -and $l -match 'Requested frame rate \[([0-9]+)fps\]') { $r.requested_fps=[int]$Matches[1] }
      if ($i -gt $con -and $l -match 'Creating encoder \[([a-z0-9_]+)\], Color coding: ([A-Z]+)') { $r.encoder=$Matches[1]; $r.color=$Matches[2]; $re++ }
      if ($i -gt $con -and $l -match 'created encoder v[0-9]+ (.+)$') { $r.encoder_flags=$Matches[1] } }
    $r.encoder_creates=$re
  }
}
if ($r.connected -and $r.client_ip) {
  $p=New-Object Net.NetworkInformation.Ping; $rtt=@()
  1..6 | ForEach-Object { $x=$p.Send($r.client_ip,800); if ($x.Status -eq 'Success') { $rtt+=[int]$x.RoundtripTime } else { $rtt+=-1 }; Start-Sleep -Milliseconds 120 }
  $r.ping_ms=$rtt
  $c=Get-Counter -Counter '\Network Interface(*)\Bytes Sent/sec','\Network Interface(*)\Bytes Received/sec' -SampleInterval 1 -MaxSamples 1
  $tx=0; $rx=0; foreach ($s in $c.CounterSamples) { if ($s.Path -like '*sent/sec') { $tx=[Math]::Max($tx,$s.CookedValue) } else { $rx=[Math]::Max($rx,$s.CookedValue) } }
  $r.nic_tx_mbps=[Math]::Round($tx*8/1e6,1); $r.nic_rx_mbps=[Math]::Round($rx*8/1e6,1)
  $top=(& $smi pmon -c 1 -s u) | Where-Object { $_ -notmatch '^#' } | ForEach-Object { $t=$_.Trim() -split '\s+'; if ($t.Count -ge 10 -and $t[3] -match '^[0-9]+$') { [pscustomobject]@{name=$t[-1]; sm=[int]$t[3]} } } | Sort-Object sm -Descending | Select-Object -First 1
  if ($top) { $r.top_gpu_process=$top.name; $r.top_gpu_sm=$top.sm }
  $sp=(Get-ItemProperty 'HKCU:\Software\Valve\Steam').SteamPath
  $cl="$sp/logs/content_log.txt"
  if ($sp -and (Test-Path $cl)) {
    $fs=[IO.File]::Open($cl,'Open','Read','ReadWrite'); $n=[Math]::Min($fs.Length,131072); [void]$fs.Seek(-$n,'End'); $b=New-Object byte[] $n; [void]$fs.Read($b,0,$n); $fs.Close()
    $since=(Get-Date).AddSeconds(-150); $rate=0
    foreach ($l in ([Text.Encoding]::UTF8.GetString($b) -split "`r?`n")) { if ($l -match '^\[([0-9-]+ [0-9:]+)\] Current download rate: ([0-9.]+) Mbps') { if ([datetime]$Matches[1] -gt $since) { $rate=[double]$Matches[2] } } }
    $r.steam_download_mbps=$rate
  }
  $r.offload_jobs=@(Get-CimInstance Win32_Process -Filter "CommandLine like '%offload-job=%'").Count
  $r.other_streamers=@(Get-Process GameViewer*,parsecd,*SunloginClient*,ToDesk* | Where-Object { $_.CPU -gt 0 } | ForEach-Object { $_.Name } | Select-Object -Unique)
}
ConvertTo-Json -Compress -Depth 3 -InputObject $r
