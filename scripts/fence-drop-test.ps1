# 收纳盒拖放自测：桌面拖文件进格子，整条链路走一遍。
#
# 前提：debug 版本正在运行（UI 自动化通道只在 debug 构建里有）。
#
# 测什么：
#   1. 每个拖放目标都登记在「泵消息的主线程」上。OLE 的规矩是 RegisterDragDrop
#      的调用线程必须自己泵消息，否则资源管理器拖着文件经过这个窗口时会一直卡在
#      DragEnter —— 用户看到的就是「从桌面拖文件进收纳盒，直接卡死」。
#      之前格子和面板都是在建窗的 worker 线程上登记的，这条检查就是那次回归的护栏。
#   2. 拖放事件送到页面后：文件真的进了收纳盒、落在光标下的子文件夹里、
#      撞名不覆盖、页面有提示、源文件不在了、应用还活着。
#   3. 事件只被**这一格**处理。Tauri 的 listen() 默认收所有目标，后端 emit_to 一格
#      的事件会广播给每个格子窗口 —— 曾经拖进 A 盒，B、C 盒会各搬一份同样的文件。
#      测试文件带随机后缀，跑完扫一遍用户目录，有一个都算失败。
#
# 真实的 OLE 拖拽没法在自动化里发起（合成鼠标事件起不了 DoDragDrop 的模态循环），
# 所以这里用 debug_fence_drag 把事件按 OLE 的样子投给格子，跳过系统那一层。
#
# 用法：在仓库根目录
#   powershell -ExecutionPolicy Bypass -File scripts\fence-drop-test.ps1
# 退出码 0 = 全过。

param(
  [int]$TimeoutSeconds = 40,
  [switch]$NoCleanup
)

$ErrorActionPreference = 'Stop'
$temp = [IO.Path]::GetTempPath()
$reportPath = Join-Path $temp 'hexbench-ui-out.txt'
$sourceRoot = 'D:\hexbench-test\fence-drop'
$logPath = Join-Path $env:LOCALAPPDATA 'io.github.hexbench\logs\HexBench.log'

$failures = New-Object System.Collections.ArrayList
$lines = New-Object System.Collections.ArrayList

function Check([bool]$ok, [string]$name, [string]$detail) {
  if ($ok) {
    [void]$lines.Add("PASS  $name")
  } else {
    [void]$lines.Add("FAIL  $name$(if ($detail) { "  ($detail)" })")
    [void]$failures.Add($name)
  }
}

function Write-Eval([string]$fileName, [string]$script) {
  Set-Content -Path (Join-Path $temp $fileName) -Value $script -Encoding UTF8
}

function Wait-Marker([string]$pattern, [int]$seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    if (Test-Path $reportPath) {
      $hit = Get-Content $reportPath -Encoding UTF8 |
        Where-Object { $_ -match $pattern } |
        Select-Object -Last 1
      if ($hit) { return $hit }
    }
    Start-Sleep -Milliseconds 250
  }
  return $null
}

function Probe-MainThread {
  $debugPath = Join-Path $temp 'hexbench-debug.txt'
  if (Test-Path $debugPath) { Remove-Item $debugPath -Force }
  Set-Content -Path (Join-Path $temp 'hexbench-probe.txt') -Value 'probe' -Encoding ASCII
  $deadline = (Get-Date).AddSeconds(5)
  while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
    if (Test-Path $debugPath) {
      $text = Get-Content $debugPath -Raw -Encoding UTF8
      if ($text -match 'main_responsive=(True|False)') { return $text }
    }
  }
  return $null
}

# 每个测试文件都带随机后缀：跑完可以在用户目录里精确地找漏网之鱼。
$tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$fileA = "hb-$tag-a.txt"
$fileB = "hb-$tag-b.txt"

# ---------------------------------------------------------------- 准备

if (-not (Get-Process hexbench-app -ErrorAction SilentlyContinue)) {
  Write-Host 'hexbench-app.exe is not running - start the debug build first.' -ForegroundColor Red
  exit 2
}

Remove-Item $reportPath -Force -ErrorAction SilentlyContinue

# 建一个自己的收纳盒，测完删掉（不碰用户已有的格子）。
Write-Eval 'hexbench-ui-main.js' @'
(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke;
  try {
    const fence = await invoke('fence_create_box', { title: 'DROP TEST' });
    await invoke('selftest_report', { text: 'DRAGTEST BOX id=' + fence.id + ' path=' + fence.path });
  } catch (error) {
    await invoke('selftest_report', { text: 'DRAGTEST BOX error=' + error });
  }
})();
'@

$boxLine = Wait-Marker 'DRAGTEST BOX ' 15
if (-not $boxLine) {
  Write-Host 'could not create the test bucket (no UI-eval answer from the main window).' -ForegroundColor Red
  exit 2
}
if ($boxLine -match 'error=(.*)$') {
  Write-Host "create bucket failed: $($matches[1])" -ForegroundColor Red
  exit 2
}
$null = $boxLine -match 'id=(\S+) path=(.+)$'
$fenceId = $matches[1]
$boxPath = $matches[2].Trim()
Write-Host "test bucket: $fenceId at $boxPath (files hb-$tag-*)"

Start-Sleep -Milliseconds 1800

# 测试数据只放在 D:\hexbench-test\ 里。
Remove-Item $sourceRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $sourceRoot, (Join-Path $sourceRoot 'same') | Out-Null
Set-Content -Path (Join-Path $sourceRoot $fileA) -Value 'first' -Encoding ASCII
Set-Content -Path (Join-Path $sourceRoot $fileB) -Value 'second' -Encoding ASCII
Set-Content -Path (Join-Path $sourceRoot "same\$fileA") -Value 'third' -Encoding ASCII
$subFolder = Join-Path $boxPath '子目录'
New-Item -ItemType Directory -Force -Path $subFolder | Out-Null

# ---------------------------------------------------------------- 页面里跑

$fenceScript = @'
(async () => {
  const id = '__ID__';
  const here = new URLSearchParams(location.search).get('id');
  if (here !== id) { return; }
  // 脚本会被重复投递（新窗口可能还没加载完，第一次注入拿到 FailedToReceiveMessage），
  // 同一窗口只跑一遍。
  if (window.__dragtestStarted) { return; }
  window.__dragtestStarted = true;
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const report = (text) => invoke('selftest_report', { text });
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  try {
    // 等页面挂好事件监听（React 的 effect 在首帧之后才跑）。
    for (let i = 0; i < 40 && !document.querySelector('.fence'); i++) { await wait(200); }
    await wait(900);

    const targets = await invoke('debug_drop_targets');
    const offMain = targets.filter((line) => line.indexOf('main=true') < 0);
    await report('DRAGTEST TARGETS total=' + targets.length + ' off_main=' + offMain.length);
    for (const line of offMain) { await report('DRAGTEST TARGET ' + line); }

    await invoke('debug_fence_drag', {
      id, kind: 'enter', paths: ['D:\\hexbench-test\\fence-drop\\__A__'], x: 220, y: 240,
    });
    await wait(250);
    await report('DRAGTEST ENTER ok');

    await invoke('debug_fence_drag', {
      id, kind: 'drop', paths: ['D:\\hexbench-test\\fence-drop\\__A__'], x: 220, y: 240,
    });
    await wait(1800);
    const toast1 = document.querySelector('.toast');
    await report('DRAGTEST DROP1 toast=' + (toast1 ? toast1.textContent : 'none'));

    const folder = Array.prototype.find.call(
      document.querySelectorAll('.tile'),
      (tile) => tile.dataset.dir === '1',
    );
    if (!folder) {
      await report('DRAGTEST FOLDER missing tiles=' + document.querySelectorAll('.tile').length);
    } else {
      const rect = folder.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      const x = Math.round((rect.left + rect.width / 2) * dpr);
      const y = Math.round((rect.top + rect.height / 2) * dpr);
      await invoke('debug_fence_drag', {
        id, kind: 'drop', paths: ['D:\\hexbench-test\\fence-drop\\__B__'], x, y,
      });
      await wait(1800);
      const toast2 = document.querySelector('.toast');
      await report('DRAGTEST DROP2 x=' + x + ' y=' + y + ' toast=' + (toast2 ? toast2.textContent : 'none'));
    }

    await invoke('debug_fence_drag', {
      id, kind: 'drop', paths: ['D:\\hexbench-test\\fence-drop\\same\\__A__'], x: 220, y: 240,
    });
    await wait(1800);
    const toast3 = document.querySelector('.toast');
    await report('DRAGTEST DROP3 toast=' + (toast3 ? toast3.textContent : 'none'));
    await report('DRAGTEST DONE');
  } catch (error) {
    await report('DRAGTEST FAIL ' + error);
  }
})();
'@

$fenceScript = $fenceScript -replace '__ID__', $fenceId
$fenceScript = $fenceScript -replace '__A__', $fileA
$fenceScript = $fenceScript -replace '__B__', $fileB
Write-Eval 'hexbench-ui-fence.js' $fenceScript

# 新格子的 WebView2 可能还没准备好，注入会失败（日志里是 FailedToReceiveMessage），
# 而脚本文件读到就删。所以没等到结果就再投一次；同一窗口由上面的标记保证只跑一遍。
$done = $null
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
while (-not $done -and (Get-Date) -lt $deadline) {
  $done = Wait-Marker 'DRAGTEST (DONE|FAIL)' 4
  if (-not $done) {
    Write-Eval 'hexbench-ui-fence.js' $fenceScript
    Start-Sleep -Milliseconds 1200
  }
}
$failLine = Wait-Marker 'DRAGTEST FAIL ' 1

# ---------------------------------------------------------------- 断言

Check ($null -ne $done) '页面跑完了整套拖放' ($(if ($done) { '' } else { '等不到 DRAGTEST DONE' }))
Check (-not $failLine) '页面里没有异常' $failLine

$targetLine = Wait-Marker 'DRAGTEST TARGETS ' 2
$offMain = 99
$totalTargets = 0
if ($targetLine -match 'total=(\d+) off_main=(\d+)') {
  $totalTargets = [int]$matches[1]
  $offMain = [int]$matches[2]
}
Check ($totalTargets -gt 0) '至少登记了一个拖放目标' "total=$totalTargets"
Check ($offMain -eq 0) '所有拖放目标都登记在主线程（不会再出现 DragEnter 卡死）' "off_main=$offMain"

$drop1 = Wait-Marker 'DRAGTEST DROP1 ' 2
Check ($drop1 -and $drop1 -match '已移入 1 项') '第一次拖入有提示' $drop1
Check (Test-Path (Join-Path $boxPath $fileA)) "$fileA 真的进了收纳盒"
Check (-not (Test-Path (Join-Path $sourceRoot $fileA))) "$fileA 的源文件已经不在了"

$drop2 = Wait-Marker 'DRAGTEST DROP2 ' 2
Check ($drop2 -and $drop2 -match '已移入 1 项') '拖到子文件夹上有提示' $drop2
Check (Test-Path (Join-Path $subFolder $fileB)) "$fileB 落进了光标下的子文件夹"
Check (-not (Test-Path (Join-Path $sourceRoot $fileB))) "$fileB 的源文件已经不在了"

$drop3 = Wait-Marker 'DRAGTEST DROP3 ' 2
Check ($drop3 -and $drop3 -match '已移入 1 项') '撞名拖入也有提示' $drop3
$sameName = @(Get-ChildItem -Path $boxPath -Filter "hb-$tag-a*.txt" -File -ErrorAction SilentlyContinue)
Check ($sameName.Count -eq 2) '撞名自动编号，没有覆盖' "找到 $($sameName.Count) 个"
if (Test-Path (Join-Path $boxPath $fileA)) {
  Check ((Get-Content (Join-Path $boxPath $fileA) -Raw).Trim() -eq 'first') "原有 $fileA 的内容没被覆盖"
}

# 别的格子 / 文件夹不能跟着搬：同一个事件只该被这一格处理。
$userDirs = @(
  (Join-Path $env:USERPROFILE 'Downloads'),
  (Join-Path $env:USERPROFILE 'Desktop'),
  (Join-Path $env:USERPROFILE 'Desktop\子模块')
)
$storeRoot = Join-Path $env:USERPROFILE 'Documents\HexBench 收纳'
if (Test-Path $storeRoot) {
  $userDirs += @(Get-ChildItem $storeRoot -Directory | Where-Object { $_.FullName -ne $boxPath } | ForEach-Object { $_.FullName })
}
$stray = @()
foreach ($dir in $userDirs) {
  if (-not (Test-Path $dir)) { continue }
  $stray += @(Get-ChildItem $dir -File -Filter "hb-$tag-*" -ErrorAction SilentlyContinue)
}
Check ($stray.Count -eq 0) '事件没有广播到别的格子 / 文件夹' (($stray | ForEach-Object { $_.FullName }) -join '; ')

$probeText = Probe-MainThread
Check ($probeText -and $probeText -match 'main_responsive=True') '拖放之后主线程还活着' $probeText

if (Test-Path $logPath) {
  $drops = @(Get-Content $logPath -Encoding UTF8 | Where-Object { $_ -match "fence fence-${fenceId}: dropped \d+ item" })
  Check ($drops.Count -ge 3) 'Rust 侧确实收到了三次 drop' "日志里 $($drops.Count) 条"
}

# ---------------------------------------------------------------- 收尾

if (-not $NoCleanup) {
  $cleanup = @'
(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke;
  try {
    await invoke('fence_delete', { id: '__ID__', mode: 'purge' });
    await invoke('selftest_report', { text: 'DRAGTEST CLEAN ok' });
  } catch (error) {
    await invoke('selftest_report', { text: 'DRAGTEST CLEAN error=' + error });
  }
})();
'@
  Write-Eval 'hexbench-ui-main.js' ($cleanup -replace '__ID__', $fenceId)
  $cleaned = Wait-Marker 'DRAGTEST CLEAN ' 10
  Check ($cleaned -and $cleaned -match 'ok') '测试收纳盒已经删掉' $cleaned
  foreach ($file in $stray) {
    Remove-Item $file.FullName -Force -ErrorAction SilentlyContinue
  }
  Remove-Item $sourceRoot -Recurse -Force -ErrorAction SilentlyContinue
  if (Test-Path $boxPath) {
    Remove-Item $boxPath -Recurse -Force -ErrorAction SilentlyContinue
  }
}

# ---------------------------------------------------------------- 汇总

$lines | ForEach-Object { Write-Host $_ }
Write-Host ''
if ($failures.Count -eq 0) {
  Write-Host 'ALL PASS' -ForegroundColor Green
  exit 0
}
Write-Host "$($failures.Count) FAILED: $($failures -join '; ')" -ForegroundColor Red
exit 1
