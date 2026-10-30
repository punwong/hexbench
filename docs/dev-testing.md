# 开发/测试用的固定环境

> 目的：自动化测试和手动验证都用同一套目录，**不要再动用户配置里的 `root` / `folders`**。

## 目录

```
D:\hexbench-test\                 ← 启动器根目录（每个子文件夹 = 一个分类）
├── 文档\          文档1..5.txt
├── 图片\          图片1.png / 图片2.png（用于验证缩略图）
├── 项目\          代码1..3.txt + 子模块\深层文件.txt（用于验证进层/返回）
├── 临时文件\      临时1..2.log
├── 界面测试\      5 个 txt + 2 张 png + 子目录\深层.txt（界面自动化用的固定夹具）
├── 大文件包\      big1..3.bin（各 100MB）
└── 压测目录\      2000 个 txt（验证流式加载 + 虚拟滚动）
```

当前配置（`%APPDATA%\io.github.hexbench\config.json`）：

| 字段 | 值 |
| --- | --- |
| `launcher.root` | `D:\hexbench-test` |
| `launcher.folders` | `D:\hexbench-test\文档`、`D:\hexbench-test\项目`、`%USERPROFILE%\Downloads` |

## 测试规则（重要）

1. **不要清空 `root` / `folders`**：用户会用它手动验证，清掉就得重新添加。
2. 需要临时换根目录时：先备份 `config.json`，测完立刻还原。
3. 需要「空目录」场景：在 `D:\hexbench-test\` 下临时建一个空子文件夹当分类，测完删掉即可，**不要改 root**。
4. 造数据（新建/复制/删除的验证）一律在 `D:\hexbench-test\` 里做，别碰用户的真实文件。

## 自动化通道

debug 构建下有个自测入口：放一个文件

```
%TEMP%\hexbench-selftest.txt
```

启动应用后会自动跑一组操作，把结果写到 `%TEMP%\hexbench-selftest-out.txt`。
它比截图可靠——直接读真实的窗口尺寸和可见性。

输出里以 `files:` 开头的行是文件区的命令级自检：列目录、新建文件夹、改名、
移动、撤销、复制 / 剪切粘贴、图标（含缓存文件是否落地）、隐藏文件过滤、
撞名自动编号，以及 1050 项目录不再截断。全部在 `%TEMP%\hexbench-fileset`
沙箱里跑，结束即删。

面板本身走的是流式接口 `launcher_items_stream`（一页 400 项推给前端），
前端再按可视范围虚拟滚动，所以大目录不再有上限。

图标走自定义协议：`http://hbicon.localhost/<16 位缓存键>`，内容来自
`%LOCALAPPDATA%\io.github.hexbench\thumbs\<键>.png`。

### 面板界面自动化（假点击打不进去时用这个）

面板是 always_on_top 的小窗口，别的程序浮在上面时系统的模拟点击（`mouse_event`）
会打到上层窗口，测不到面板。改用应用内的脚本通道：

```
把脚本写到 %TEMP%\hexbench-ui.js  →  应用在面板里 eval 执行
脚本里用 window.__TAURI_INTERNALS__.invoke("selftest_report", { text })
                                    →  追加到 %TEMP%\hexbench-ui-out.txt
```

只在 debug 构建里有：`spawn_ui_eval` 每 250ms 看一眼脚本文件，执行完就删。
脚本里可以直接派发 `PointerEvent` / `MouseEvent` / `input` 事件，测的就是前台
真实的 React 处理器；读 DOM 拿渲染了多少项、选中了什么、撑开多高。

注意两点：

1. 选中/拖动是 `pointerdown` 起手的，派发完要补一个 `pointerup`，否则 window 上
   的拖拽监听会留着，下一次 `pointermove` 会真的发起一次 OLE 拖拽。
2. 框选会调 `setPointerCapture`，脚本里可以先把它换成空函数（pointerId 是造的）。
3. 面板窗口是第一次呼出时才建的：应用刚启动、还没按过 Alt+1 就写脚本，日志里只有
   `ui-eval: launcher missing`，脚本已经被吃掉——先按一次 Alt+1 再写。
4. 脚本里写 Windows 路径别经过 shell（`echo` / heredoc 会吃掉一层 `\\`，`\h` 就对不上了）。
   用编辑器写文件，或者拿 `el.dataset.path.endsWith(...)` 比，不拼完整路径。

不启动应用也能跑的一层在 `crates/hexbench-core/tests/file_ops.rs`：

```
cargo test -p hexbench-core --test file_ops -- --test-threads=1 --nocapture
```

串行是必须的：剪贴板和回收站是系统级状态。

## 呼出 / 收起的压测

卡死过的两次都在这里，改完 `reveal_launcher` / `hide_launcher` 都要跑一遍：

```
连按 Alt+1 40～60 次，间隔 40～80ms
```

- 用 `keybd_event` 发，不用模拟鼠标点击（面板是 always_on_top，点击容易打到别的窗口）。
- 判定：每次按键后 `IsWindowVisible` 应严格 1/0 交替；日志里不应出现
  `ui: main thread busy`（看门狗 ≥400ms 就会记一行，后面括号里是当时卡住的操作）。
- 最后再按一次应能正常呼出，并且 `GetForegroundWindow` 就是面板窗口（说明能打字）。

**别踩的坑**：窗口操作（`show` / `hide` / `set_focus` / `SetForegroundWindow`）
只能在窗口自己所属的线程上做。从别的线程调 `SetFocus` 是跨线程同步发
`WM_KILLFOCUS` / `WM_SETFOCUS`，主线程正好在处理键盘消息（tao 在里面持有
`KEY_EVENT_BUILDERS`）时互等，就是之前那种「连按几次后整块面板僵死、快捷键失效」。
需要延迟执行就 `sleep` 在工作线程、`run_on_main_thread` 回主线程。

## 右键菜单里的系统命令（压缩 / 复制 / 发送到）

面板菜单里选完系统命令后，`InvokeCommand` 只是把活儿交给 Shell，命令自己还会弹
进度窗口抢前台。这段时间面板必须留着：

- `show_shell_context_menu` 在 `InvokeCommand` **之前**就置上 `SHELL_OP_UNTIL`
  （`_ms` 时间戳），`watch_shell_op` 盯着目标目录的条目数 / 最新修改时间 / 总大小，
  还在变就续期；10 次轮询（2.5 秒）没动静就算干完了。
- 期间收到的失焦不收起面板，改由 `hide_after_shell_op` 在命令结束后补一次收起
  （前提是那时前台确实不在面板上）。

复现/验证：在 `D:\hexbench-test\` 里临时放几百 MB 的目录，右键 →「添加到 xxx.zip」，
日志里应看到 `invoked shell command` 之后**没有**紧跟着 `hide`，压缩结束后才收。
自动化可以用 `%TEMP%\menutest.ps1`（弹菜单、按 Down×N + Enter 选中）。

## 验证常用手法

| 目的 | 手法 |
| --- | --- |
| 快捷键是否触发 | 看日志里的 `clipboard:` / `trash:` / `bg menu:` 行 |
| 文件操作是否成功 | 直接 `Get-ChildItem` 看磁盘 |
| 窗口尺寸/位置 | 枚举窗口 + `GetWindowRect` |
| 刷新是否及时 | 操作后截图看列表，或读日志 |
| 主线程卡在哪 | 看 `ui: main thread busy … (stuck in …)`，对应 `lib.rs` 里的 `op_name` |
