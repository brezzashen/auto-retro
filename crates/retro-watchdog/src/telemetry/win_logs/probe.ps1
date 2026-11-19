$ErrorActionPreference='SilentlyContinue'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
$cur=ConvertFrom-Json '__CURSOR__'
$boot=@(12,13,41,109,1074,6005,6006,6008)
$routine='Accepted publickey|disconnected by user|^sshd: Disconnected from|^sshd: Received disconnect'
$dnoise='event streamer: error: EOF|traces export: rpc error|BackendAPI S->C|ErrorReportAPI S<-C|Failed to fire hook|dxgkio_query_adapter_info|hostPathOfVolume|"level":"info"|settings store state|Registry Access Management'
$iso={ param($t) $t.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ss.fffZ') }
$one={ param($m) $m=("$m" -replace '\s+',' ').Trim(); if($m.Length -gt 400){$m=$m.Substring(0,400)}; $m }
$ev=[Collections.ArrayList]::new(); $sl=[Collections.ArrayList]::new(); $dl=[Collections.ArrayList]::new()
$o=[ordered]@{events=$ev; ssh=$sl; minutes=[ordered]@{}; docker=$dl; cursor=[ordered]@{}; counts=[ordered]@{}}
foreach($log in 'System','Application','OpenSSH/Operational'){
  $after=[int64]$cur.$log; $last=$after; $n=0
  $xp="*[System[EventRecordID>$after and TimeCreated[timediff(@SystemTime) <= __BACKFILL_MS__]]]"
  foreach($e in @(Get-WinEvent -LogName $log -FilterXPath $xp -Oldest -MaxEvents __MAX__)){
    $last=[int64]$e.RecordId; $n++; $t=& $iso $e.TimeCreated
    if($log -eq 'OpenSSH/Operational'){
      if($e.Message -like '*Accepted publickey*'){ $k=$t.Substring(0,16); $o.minutes[$k]=1+[int]$o.minutes[$k] }
      elseif($e.Message -notmatch $routine){ [void]$sl.Add(@($t,(& $one $e.Message))) }
    } elseif($e.Level -in 1,2,3 -or $e.Id -in $boot){
      [void]$ev.Add(@($t,$log,[int]$e.Id,[int]$e.Level,("$($e.ProviderName)" -replace '^Microsoft-Windows-',''),(& $one $e.Message)))
    }
  }
  $o.cursor[$log]=$last; $o.counts[$log]=$n
}
$since="$($cur.docker)"; $newest=$since
Get-ChildItem "$env:LOCALAPPDATA\Docker\log\host" -File | Where-Object { $_.Name -match '^(com\.docker\.backend\.exe|monitor)\.log' -and $_.LastWriteTimeUtc.ToString('yyyy-MM-ddTHH:mm:ss') -ge $since.Substring(0,[Math]::Min(19,$since.Length)) } | ForEach-Object {
  foreach($l in [IO.File]::ReadLines($_.FullName)){
    if($l -notmatch '^\[(\d{4}-\d\d-\d\dT[\d:.]+Z)\]'){ continue }
    $ts=$Matches[1]; if($ts -le $since){ continue }; if($ts -gt $newest){ $newest=$ts }
    if($l -match '(?i)error|fail|timeout|refused|reset|unhealthy|panic|fatal|restart|stopp|deadline' -and $l -notmatch $dnoise -and $dl.Count -lt 2000){ [void]$dl.Add(@($ts,(& $one $l))) }
  }
}
$o.cursor.docker=$newest
$os=Get-CimInstance Win32_OperatingSystem; $cpu=Get-CimInstance Win32_Processor | Select-Object -First 1
$o.host=[ordered]@{cpu="$($cpu.Name)".Trim(); logical=[int]$cpu.NumberOfLogicalProcessors; boot=(& $iso $os.LastBootUpTime)}
ConvertTo-Json -Compress -Depth 5 -InputObject $o
