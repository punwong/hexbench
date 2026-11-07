# 开发与测试指南

本文说明快捷面板、桌面格子、收纳盒和便签的自动化测试与手动验证方法，包括固定测试目录、自动化入口、各项功能的验证步骤和判定方法。

测试数据统一放在 `D:\hexbench-test\`，测试进程使用独立配置，不修改用户配置中的 `root` / `folders`。

## 目录

- [测试目录](#测试目录)
- [测试数据规则](#测试数据规则)
- [自动化入口](#自动化入口)
- [便签](#便签)
- [删除 / 回收站后的键盘焦点](#删除--回收站后的键盘焦点)
- [呼出 / 收起的压测](#呼出--收起的压测)
- [右键菜单里的系统命令（压缩 / 复制 / 发送到）](#右键菜单里的系统命令压缩--复制--发送到)
- [图标（lucide 一套）](#图标lucide-一套)
- [调试版 exe 的前端来源](#调试版-exe-的前端来源)
- [验证常用手法](#验证常用手法)

## 测试目录

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

## 测试数据规则

1. **不要清空 `root` / `folders`**：用户需要用它们手动验证，清空后需要重新添加。
2. 需要临时更换根目录时：先备份 `config.json`，测试完成后立即还原。
3. 需要「空目录」场景时：在 `D:\hexbench-test\` 下临时新建一个空的子文件夹作为分类，测试后删除，**不要修改 root**。
4. 新建、复制、删除等造数据操作一律在 `D:\hexbench-test\` 内进行，不要改动用户的真实文件。

## 自动化入口

debug 构建提供自测入口：写入文件

```
%TEMP%\hexbench-selftest.txt
```

启动应用后会自动执行一组操作，并将结果写入 `%TEMP%\hexbench-selftest-out.txt`。该方式直接读取真实的窗口尺寸和可见性，比截图判定更可靠。

输出里以 `files:` 开头的行是文件区的命令级自检：列目录、新建文件夹、改名、
移动、撤销、复制 / 剪切粘贴、图标（含缓存文件是否落地）、隐藏文件过滤、
撞名自动编号，以及 1050 项目录不再截断。全部在 `%TEMP%\hexbench-fileset`
沙箱里跑，结束即删。

面板本身走的是流式接口 `launcher_items_stream`（一页 400 项推给前端），
前端再按可视范围虚拟滚动，所以大目录不再有上限。

图标走自定义协议：`http://hbicon.localhost/<16 位缓存键>`，内容来自
`%LOCALAPPDATA%\io.github.hexbench\thumbs\<键>.png`。

### 面板界面自动化

面板是 always_on_top 的小窗口，其他程序浮在其上时，系统模拟点击（`mouse_event`）会落在上层窗口，无法测到面板。此时使用应用内的脚本通道：

```
把脚本写到 %TEMP%\hexbench-ui.js  →  应用在面板里 eval 执行
脚本里用 window.__TAURI_INTERNALS__.invoke("selftest_report", { text })
                                    →  追加到 %TEMP%\hexbench-ui-out.txt
```

该通道只在 debug 构建中可用：`spawn_ui_eval` 每 250ms 检查一次脚本文件，执行后删除。脚本可以直接派发 `PointerEvent` / `MouseEvent` / `input` 事件，触发的是前台真实的 React 处理器；通过读取 DOM 可以确认渲染项数、选中状态和撑开高度。

注意事项：

1. 选中和拖动以 `pointerdown` 开始，派发后需要补一个 `pointerup`；否则 window 上的拖拽监听会保留，下一次 `pointermove` 会真正发起一次 OLE 拖拽。
2. 框选会调用 `setPointerCapture`，脚本中可以先将它替换为空函数（pointerId 由脚本构造）。
3. 面板窗口在第一次呼出时才创建。应用刚启动、尚未按过 Alt+1 时写入脚本，日志中只有 `ui-eval: launcher missing`，脚本会被删除。应先按一次 Alt+1 再写入。
4. 脚本中写入 Windows 路径时不要经过 shell（`echo` / heredoc 会丢失一层 `\\`，`\h` 将无法匹配）。应使用编辑器写入文件，或通过 `el.dataset.path.endsWith(...)` 比较，不拼接完整路径。

### 桌面格子的界面自检

格子是原生窗口（`crates/hexbench-fence`），没有可注入脚本的页面，内容、视图和锁定位置只能从外部观察：截图、`debug_native_state`（窗口清单、z-order）和 `debug_native_drop_target`（落点）。新建和删除格子仍然通过命令完成，等同于右键菜单点击「新建格子…」之后的步骤（菜单只负责把路径交给 `fence_create`）：

```js
await window.__TAURI_INTERNALS__.invoke("fence_create", { path: "D:\\hexbench-test\\文档" });
await window.__TAURI_INTERNALS__.invoke("fence_delete", { id: "<上一步返回的 id>" });
```

日志中出现 `fence …: created` 不能证明格子显示正常。应先调用 `Shell.Application.MinimizeAll()` 露出桌面再截图，标题条和条目绘制出来才算通过。格子最大化或被其他窗口覆盖时，截图得到的是其他窗口。

外观（不透明度、磨砂）只能通过像素采样验证：

```js
await window.__TAURI_INTERNALS__.invoke("fence_set_appearance", { opacity: 40, frosted: false });
```

- 磨砂开着：卡片平均亮度比纯壁纸高（DWM 亚克力自带一层很淡的白）。
- 磨砂关掉：卡片亮度≈`不透明度 × 卡片色` + `(1 − 不透明度) × 壁纸`。
  深色壁纸上 60% 约 150、100% 约 239，差得足够分辨。
- 像素采样前先 `Shell.Application.ToggleDesktop()` 收起别的窗口：设置窗口压在格子上时
  量到的是设置窗口的白底（实测 238，和 100% 不透明度几乎一样，很容易判错）。

**z-order 和拖动只能从外面看。** 判定「格子贴着桌面」：

- 取格子的 `GetWindowLongPtr(hwnd, GWL_EXSTYLE)`：`0x8`（`WS_EX_TOPMOST`）必须没有——
  有它就是置顶面板，盖住了所有普通窗口。
- 从 `GetTopWindow` 按 `GW_HWNDNEXT` 往下数：格子应该紧挨在 `GetShellWindow()`（Progman）
  上面一位，而且排在任何普通窗口（比如别的应用）下面。
- 按一次 `Shell.Application.ToggleDesktop()`（相当于 Win+D）再按回来，这两个判断都要还成立。
- 拖动抖动：按住标题条往屏幕右下角拖，每 25ms 采一次 `GetWindowRect`。位置应该跟着光标
  1:1 走、到工作区边缘停住；松手之后**第一帧的矩形不能变**（以前会往回跳二十几个像素）。

### 格子的右键手势

手势和快捷面板共用一份设置（`launcher.gestures`），格子这边是 `win.rs` 里的右键状态机：
`WM_RBUTTONDOWN` 只记起点，横向位移超过 8px 且横向占优才判成手势（这时才 `SetCapture`），
松手时位移够 `triggerDistance` 才发 `Event::Back` / `Event::Forward`，历史在 `NATIVE_NAV`
（根 + 走过的目录，最多 50 层）。用 `mouse_event`（`0x0008` 按下、`0x0010` 松开）从格子
正文里划：

- 划动过程中截图应看到轨迹线和光标旁的「← 后退 / → 前进」；历史到头时提示画淡一档，
  松手不动作，而且这一次右键**不弹菜单**。
- 轨迹线的画法和面板的 canvas 版对齐：整条轨迹建成**一条**路径（`gesture_path`），
  经过相邻采样点的中点画二次曲线，辉光 9px / 主线 3px 各描一次边。不要改回逐段
  `DrawLine`：每段单独绘制会在每个采样点重复叠加半透明，拐角处出现深浅不一的斑点和
  折角，导致轨迹比面板粗糙。验收时将划出的线截图放大 4–6 倍：线上不应有网格状接缝，
  辉光边缘应保持连续。
- 没划够 8px 的普通右键必须照常弹菜单（回归点：手势判定之后才抢捕获）。
- 划回上一层后标题条的面包屑应退一段；反方向划回来应恢复。
- 只在正文起手（`y >= HEADER_H`），标题条上右键不认手势。

### 格子的拖动 / 缩放 / 菜单 / 拖出

格子自己发 `Event::BeginMove` / `Event::BeginResize`（带 `hwnd` 和 `scale`），后端
`fence_run_move` / `fence_run_resize` 负责吸附、参考线、松开落盘，所以上面「吸附参考线」
那套验法原样适用。窗口过程这边只做
命中判定，移动 / 缩放本身靠后端 8ms 的轮询循环，窗口自己的 `WM_MOUSEMOVE` 不参与。

- 标题条起拖（`y < HEADER_H`，三颗点和面包屑除外）；正文条目上按住移动 4px 起的是
  文件拖拽，不是挪窗口。
- 边角：四角 10px、四边 5px（逻辑像素，`edge_at`）。`WM_SETCURSOR` 在 `HTCLIENT` 上
  按方向换 `IDC_SIZEWE/NS/NWSE/NESW`；卷起时只认左右边。
- 后端循环直接 `SetWindowPos`，尺寸变化来自 `WM_SIZE`：更新 `px_w/px_h`、重建
  DirectComposition surface、再按新宽高重排（列数、滚动范围都跟着变），不需要等一帧
  新数据。实测拉右下角 384×206 → 444×252，网格从 4 列自适应，松手后配置里的归一化
  矩形同步更新；拉左边 / 上边时窗口原点跟着走（262,133 404x222）。
- 标题菜单按 `Frame` 里的 `is_bucket` / `rule_count` / `sort_key` / `sort_dir` 生成：
  文件夹是「更换文件夹… / 在资源管理器中打开」，收纳盒是「打开收纳目录 / 立即整理
  （有规则时）」；当前排序键后面带 ↑ / ↓，再点同键反转方向（配置 `sortKey/sortDir`）。
- 标题条右侧的计数数字换成了**视图切换图标**（原来是文件数）：现在是图标网格就画列表
  （三条线），现在是列表就画网格（四个方块），点一下在 `Frame.list` 的 `true` / `false`
  之间切，行高 28px、图标 18px。它占 `DOTS_W + 6` 到 `DOTS_W + 6 + VIEW_W` 这一段，
  点在这里不启动拖动、双击也不卷起。设置页对应行的小按钮是同一份状态。
- 多选：`picked` 存的是**路径**（重读目录、过滤之后都还对得上），`shown` 存的是显示顺序，
  所以命中 / 高亮 / 框选 / 拖出全部用「显示位置」索引，`items` 本身不动。Ctrl 点选、
  Shift 从锚点连选、空白处拖 `Band`（相交即选，按 Ctrl 时叠在按下前的底子上）。
  按在已选中的一项上先不缩成单选，松手才缩（`collapse`）；真拖起来就把整组交给
  `start_file_drag(&paths)`。右键菜单走 `sys::show_shell_context_menu_for`，只把
  **同一父目录**的条目塞进一份菜单；多选时不插自绘的「重命名」。验证时选中两项再右键，
  菜单里应没有「重命名」；删除只会动自己的测试目录（删到回收站）。
- 搜索：标题条放大镜占 `view_left - 6 - SEARCH_W` .. `view_left - 6`，点开是贴着标题条
  的弹层（窗口类 `HexBenchNativeFenceSearch` + 系统 `EDIT`，和改名框共用
  `create_edit_popup`）。`WM_CHAR` / `WM_KEYUP` / `WM_PASTE` / `WM_CUT` / `WM_CLEAR`
  之后 `apply_search` 重算 `shown`；回车只收框、Esc 清空再收，失焦也收（筛选留着）。
  放大镜本身在筛选非空时用正文色。弹层是格子的子弹层，格子 `WM_NCDESTROY` 时一起销毁。
  验证：输入一个只匹配一条的字，回车后双击那一条——必须打开 / 进入匹配到的那一项；
  进子文件夹后筛选词仍在，空结果应显示「没有匹配的条目」而不是「这里是空的」。
  打字用剪贴板 + `SendKeys("^v")` 送中文；`shot.ps1` 会 MinimizeAll，先打完字再截图。
- 列表信息列：`list_cols()` 给出名称列结束 / 时间列起点 / 大小列起点，全部按 zoom 缩放。
  时间今年只写「月-日 时:分」、往年带年（`local_offset_secs` 取一次时区偏移），文件夹的
  大小留空。列名条固定在 `HEADER_H .. HEADER_H + LIST_HEAD_H`，条目在它下面滚动
  （`paint` 里给正文区单独 `PushAxisAlignedClip`，否则滚上来的行会盖住列名）。点列名发
  `Event::Sort`，同键反转，箭头画在列名后面。行与行之间那 2px `LIST_GAP` 也算命中
  （算上面那一行），不应留成点击后清空选中的死区。
- 托盘「显示全部格子」：把所有 `visible = false` 的格子置回可见、存盘、
  `apply_fence_windows`。没有隐藏的格子时什么也不做（不写盘）。验证：用测试格子
  （`D:\hexbench-test` 里随便建一个）的「⋯ → 隐藏格子」，`EnumWindows` 里应少一个
  `HexBenchNativeFence`、配置里 `visible: false`；再点托盘这一项，窗口回来、`visible: true`。
  托盘图标在 UIA 里叫 `SystemTray.NormalButton`、名字 `HexBench`（坐标取
  `BoundingRectangle` 中心）。
- 菜单最后一项是「锁定位置 / 解锁位置」（`Frame.locked`）。锁定后标题条拖不动、
  边角光标也不出现，但双击卷起、面包屑、视图按钮、菜单照常。
- 条目的拖出用 `hexbench_core::sys::start_file_drag`，**在渲染线程上调**，起手前
  `SetCapture` 这个窗口：`DoDragDrop` 要和自己窗口同线程的捕获。曾经改到主线程
  `run_on_main_thread` 上跑，主线程被 `DoDragDrop` 卡住、`ui: main thread busy` 一直
  报，松手也不返回——落点还收不到文件。落回自己的格子 / 另一个格子 / 资源管理器都走
  OLE 那一路；`Event::Drop` 里会先挡掉「拖进自己所在目录」的空操作。
- 提示 toast 画在格子底部，`NativeFences::toast(id, text)` 推给渲染线程，
  `TIMER_TOAST` 2.6 秒后自己消失。拖入成功 / 规则整理结果 / 失败信息都走它。
- Win+D / 显示桌面：原生窗口是 `WS_POPUP + WS_EX_TOOLWINDOW` 的桌面层窗口，Show Desktop
  根本不给它发 `SC_MINIMIZE`（去掉 `WS_EX_TOOLWINDOW` 试过，同样不发），所以只能靠 z-order
  判断。资源管理器抬起来的是桌面窗口本身
  （24H2 之后是 `Progman`，更早的版本是图标所在的 `WorkerW`），格子被压在它下面。
  `fence_watchdog` 靠两件事认出这一刻：某个格子的 z-order 掉到了桌面宿主下面
  （`behind_desktop`），并且前台是 shell 自己的窗口（`foreground_is_shell`：桌面、
  任务栏、桌面右键菜单都属于 Explorer 的进程），或者宿主已经在所有普通窗口前面
  （`desktop_raised_over_windows`——抬桌面和换前台之间有一小段空档，只看前台会漏）。
  `keepOnShowDesktop` 为真时照常 `anchor_above_desktop` 把格子插回桌面上方；为假时
  整轮跳过，格子留在后面被桌面盖住，也就是「跟着桌面一起藏」。别的程序回到前台后
  桌面落回底层，格子自然又露出来，不需要单独记「结束」时刻。
  验证时把配置里 `keepOnShowDesktop` 改成 `false` 重启：按 Win+D 应只剩桌面图标，
  再按一次（或点任务栏 / 点程序）格子回来；改成 `true` 时 Win+D 之后格子仍可见。
  判断状态时不要通过截图数像素：`EnumWindows` 按 z-order 从上往下枚举，格子的索引小于
  `Progman` 即表示位于其上方。日志中 `fence: 桌面盖住格子…` / `桌面不再盖住格子…` 各出现一行。
- 重命名：标题菜单「重命名格子」和条目右键「重命名」弹的是同一个原生弹层（窗口类
  `HexBenchNativeFenceRename` + 系统 `EDIT`，输入法 / 选区 / 剪贴板都交给控件），
  回车提交、Esc 取消、点到别处也取消。标题默认全选；文件预填名字并只选中主干部分
  （扩展名留在外面，和资源管理器一致），文件夹则是全选。提交走 `Event::Rename` /
  `Event::RenameItem`，后者经 `launcher_rename` 改名并重读目录；一个字没改不发事件。
  验证时用 `keybd_event` 发送 `VK_RETURN`（`0x0D`）/ `VK_ESCAPE`（`0x1B`）；WScript 的
  `SendKeys("{ENTER}")` 在同一个脚本中经常无法送达，不能作为判定依据。改名只在测试
  目录中进行：条目菜单中「重命名」与「删除」相邻，点击偏移会实际删除文件。
- 表头背景：`paint` 里表头的圆角矩形要画到 `HEADER_H + RADIUS`，才能把下面两个圆角
  压成直边；但必须 `PushAxisAlignedClip` 裁到表头高度。不裁的话展开后会有一条 8px
  的表头色带露在分割线下面，看着就像「顶部下方多了一点 UI」。验收时截格子顶部，沿一条
  竖线数像素：分割线（比表头暗一点的 1px）下面必须直接是正文底色。
- 滚动后的拖放落点：`debug_native_drop_target(id, x, y)` 按 OLE 回调**同一份**口径
  （`DropSnapshot::destination`）算一次落点，只读不搬，`x` / `y` 是客户区物理像素。
  铺 40 多个文件让格子能滚，先量一个压着文件夹的点，再往格子里 `PostMessage`
  一条 `WM_MOUSEWHEEL`（`0x020A`，wParam 高 16 位是滚动量）滚到底，同一个点应改成
  返回当前目录（或滚上来之后压着的那个文件夹）。如果快照只在排版时重建、滚轮只修改
  `scroll`，高亮位置会更新，松手却按上一屏的位置落点。不要用真实滚轮测试：格子是
  桌面层窗口，前台一旦盖住它，滚轮事件会发送到其他窗口。

### 格子的面包屑

标题条进了子文件夹以后是 `.head .crumb`（每段一个 `data-crumb`，值是那一段的完整路径），
停在绑定文件夹本身时没有面包屑、只有 `.head .title`。注入 fence 脚本 `dblclick` 一个
`.tile[data-dir="1"]` 之后读 `document.querySelectorAll('.head .crumb').length`，
应等于「根 + 进去的层数」；点第一段（根）应回到根目录、`.title` 重新出现。

拖到某一段上松手就落在那一层：`debug_fence_drag` 先发 `over`，那一段应带上
`.crumb.drop-hover`；再发 `drop`，`launcher_move_into` 会把文件移到那一段的目录里
（实测从 `界面测试\子目录` 拖到根，文件落在 `界面测试` 下）。

脚本中写入 Windows 路径时使用正斜杠（`D:/hexbench-test/...`）。通过 heredoc 写入
`%TEMP%\hexbench-ui-*.js` 时，`\\` 会丢失一层，反斜杠在 JS 中变为转义符，
`'D:\\hexbench-test'` 最终成为 `D:hexbench-test`，`fence_create` 报「文件夹不存在」。

### 吸附参考线（跨屏虚线）

虚线不属于任何格子，由独立窗口 `snap-guides` 绘制：覆盖整个虚拟屏幕、透明、置顶、点击穿透、不进入任务栏。启动时预先创建并隐藏（`prebuild_guides`），拖动时显示，松手后隐藏。
格子页面中既没有这条线，也没有对应事件，只能从外部验证：

1. 用 `mouse_event` + `SetCursorPos` 按住某个格子的标题条拖，停在离对齐位 8px 以内
   （吸附阈值，DIP×DPI）。中途照片要在**还按着**的时候拍。
2. 截屏后按颜色找线：`B > R + 40` 且 `B > 150` 的像素，按列 / 按行统计，命中最多的那一列
   （或那一行）就是竖线（横线）的位置，应该正好是被对齐的那条边。
3. 松手后再拍一张，同样的列 / 行必须归零；`HGUIDE` 画布窗口同时要被藏回去。

以下为三条注意事项：

- **运行中新建的窗口没有 webview**（只有外壳），页面不会加载。因此画布必须在启动时预先创建；在拖动过程中延迟创建将无法收到任何绘制指令。
- 新窗口的 label 必须写入 `src-tauri/capabilities/default.json` 的 `windows`，否则页面中的 `listen` 会被权限拒绝（Promise 静默失败），表现为画布窗口和页面都存在，但没有任何线条出现。修改后需要重新 `cargo build`，capability 会编译进二进制。
- 画布是隐藏窗口，外层窗口标题**不会**随 `document.title` 变化。确认页面是否加载时，应枚举其子窗口，查看 `Chrome_WidgetWin_1` 子窗口的标题（页面中设置为 `HGUIDE v/h/-`）。

调试通道：`%TEMP%\hexbench-ui-guides.js` 会被执行到画布窗口（日志中为 `ui-eval: guides …`）。

另外：测试用的格子**不要与用户已有的格子重叠**。重叠区域内的鼠标事件会落到 z-order 更高的窗口，表现为无法拖动；应先移开或换一个空位再测试。

### 收纳盒与整理规则

收纳盒（`kind: "bucket"`）没有用户选的文件夹：`fence_create_box` 在「文档\HexBench 收纳」
下按 `<标题>-<id 后 10 位>` 建目录，`path` 存的就是这个目录，所以列目录 / 拖入 / 拖出
全都走原来那条路径，不用另测一遍。测规则时把来源指向 `D:\hexbench-test\` 里的目录，
**不要指向真实的下载目录**：条件写错会把整个目录纳入扫描。

```js
// 建盒 → 塞规则 → 预览 → 跑
const box = await window.__TAURI_INTERNALS__.invoke("fence_create_box", { title: "测试收纳盒" });
await window.__TAURI_INTERNALS__.invoke("fence_update", {
  id: box.id,
  rules: [{ id: "r1", name: "发票", enabled: true, exts: ["pdf"], nameContains: "",
            minMb: 0, maxMb: 0, olderThanDays: 0,
            sources: ["D:\\hexbench-test\\来源"], subdir: "文档" }],
});
await window.__TAURI_INTERNALS__.invoke("fence_rule_preview", { id: box.id }); // 只读
await window.__TAURI_INTERNALS__.invoke("fence_rule_run", { id: box.id });     // { matched, moved }
await window.__TAURI_INTERNALS__.invoke("fence_box_info", { id: box.id });     // { store, items, bytes }
await window.__TAURI_INTERNALS__.invoke("fence_delete", { id: box.id, mode: "purge" });
```

- 规则的 `exts` 会被 `clamp_rules` 规整（去掉点、转小写、丢非法字符），大小 / 天数也会夹住；
  建完再 `fence_state` 读回来，确认存下去的是规整后的值。
  规则 id 只在空着的时候由后端补（`r<时间><序号>`），前端生成的 `r…` 会原样存下去。
- **不能自吞**：第一次 `fence_rule_run` 之后立刻再预览一次，必须是 0——盒子目录和所有
  收纳目录都在排除表里，子目录里的文件不能被下一轮当成新文件再收一遍。
- 来源要一条一条扫：深度只有 1 层（来源目录本身），隐藏 / 系统文件跳过，单次上限 5000。
- 自动整理走 watcher：往来源里扔一个匹配的文件，等 1.5 秒防抖 + 一轮扫描（实测约 3 秒），
  盒子里的条目数就会变；应用刚启动时也会先跑一遍（实测约 6 秒）。
  关掉 `rulesAuto` 后同样的文件应该一直躺在来源里。
- 删除的三种模式都试一遍：`release` 移到桌面、`purge` 进回收站、`keep` 留在收纳目录；
  之后 `Documents\HexBench 收纳` 下不应该留下空目录。
- 界面这一层用 `%TEMP%\hexbench-ui-main.js`：点导航「桌面格子」→ 新建收纳盒 →
  点行里的「整理规则」图标。新加的规则**默认没有来源**，卡片里应该出现
  「还没有来源，这条规则不会收任何文件」；给上来源但不填任何条件时，应该换成
  「没有条件：这些来源里的所有文件都会被收进来」的提醒。

不启动应用也能跑的一层在 `crates/hexbench-core/tests/file_ops.rs`：

```
cargo test -p hexbench-core --test file_ops -- --test-threads=1 --nocapture
```

串行是必须的：剪贴板和回收站是系统级状态。

### 拖放：从桌面拖文件进收纳盒

拖放目标由 OLE 按窗口登记。**OLE 只将回调交给调用 `RegisterDragDrop` 的线程，并且该线程必须自行处理消息循环**；否则任何拖拽经过该窗口的程序都会一直卡住，直到目标程序退出（见 MSDN 的 `RegisterDragDrop` 说明）。原生格子的窗口和拖放目标都位于渲染层自己的消息循环线程（`crates/hexbench-fence`），该线程本身处理消息；呼出面板的窗口属于主线程，登记通过 `run_on_main_thread` 完成。若在创建窗口的 worker 线程上登记，从桌面拖动文件进入收纳盒时整个拖拽会冻结。

格子没有可注入脚本的页面，自动化只能观察窗口和落点：

| 命令 | 用途 |
| --- | --- |
| `debug_native_state` | 列出原生格子的窗口：`hwnd` / `visible` / `topmost` / `behind_desktop` / `rect`。 |
| `debug_native_drop_target` | 按 OLE 回调相同的规则计算落点（只计算，不移动）：传入格子 id 和客户区物理坐标，返回该点下方的目录。用于回归验证滚动后落点错误的问题。 |
| `debug_drop_targets` | 面板侧拖放目标的登记线程：`label` / `hwnd` / `registered_tid` / `owner_tid` / `main`。**`main=true` 表示登记正确**。 |

真实的 OLE 拖拽无法在脚本中发起，但可以用 `mouse_event` + `SetCursorPos` 模拟：资源管理器自行进入 `DoDragDrop`，格子收到的是真实 OLE 回调（日志 `drag: enter` → `drop`）。
该方式需要人工确定坐标，因此未纳入回归脚本。编写脚本时应先按 Win+D 露出桌面（被窗口覆盖时会拖动该窗口），落点取格子的 `pixelX/pixelY` 中心。

**修改拖放代码后必须人工拖拽一次**：桌面 → 收纳盒空白处、桌面 → 盒子内的子文件夹、
盒子 → 资源管理器、两个盒子互拖；松手前光标应始终跟随，资源管理器不能卡顿。

### 拖放：浏览器网页里的图片（虚拟文件）

从 Chrome / Edge 的网页里拖图片，数据对象里**没有 `CF_HDROP`**：只有
`FileGroupDescriptorW`（文件名）和 `FileContents`（`IStream`）。实测枚举到的格式：

```
DragContext | DragImageBits | chromium/x-renderer-taint | chromium/x-drag-id |
FileGroupDescriptorW | FileContents | text/x-moz-url | UniformResourceLocatorW |
UniformResourceLocator | UnicodeText | Text | HTML Format | text/html
```

所以「只认 `CF_HDROP`」的拖放目标会看到空路径列表：光标和落点高亮都正常，松手却什么都
不搬。`drop_target::drop_paths` 在 `CF_HDROP` 为空时退回虚拟文件：按描述符里的名字建临时
文件（`%TEMP%\hexbench-drop\<pid>-<时间戳>\`），把流读到 EOF，再交给原来的搬运流程；一小时
后残留的空目录会在下一次拖放时清掉。

回归测试（不需要浏览器）：

```
cargo test -p hexbench-core drop_target
```

测试里造了一个假 `IDataObject`，只答 `FileGroupDescriptorW` + `FileContents`，断言内容
逐字节一致、中文文件名保留、`..\\` 之类的名字被清理。手工验证则用 Edge 打开一张本地图片，
拖到面板空白处，文件应出现在面板当前目录。写探针时注意：`STGMEDIUM.hGlobal` 是**句柄**，
要先 `GlobalLock` 再读，直接当指针读会得到垃圾名字。

## 便签

**测试前先将便签目录改到测试盘**：默认目录是用户真实的「文档\HexBench 收藏」。验证时在测试配置里把 `notes.folder` 设为 `D:\hexbench-test\notes`（或在设置页选择该目录），不要在真实文档目录中写入测试笔记。

- 新建、编辑、改名、删除、清空各验证一遍。改名会同步修改文件名（重名时自动追加 `(2)`），列表中的 id 随之变化，选中状态和后续编辑不能落到旧文件上。
- 点击卡片空白处（标题、预览，不含按钮）进入编辑；点击「发送 / 复制图片 / 删除」不应同时打开编辑器。双击卡片时，第二次点击落在刚弹出的遮罩上，编辑器不能闪退。
- 磁盘上一篇笔记对应一个 `.md` 文件，正文为 Markdown（没有 frontmatter），图片位于 `images/` 下，正文引用写作 `![名字](<images/文件名>)`；内容相同的图片只保存一份。
- 拖图：从资源管理器拖到空白处新建，拖到卡片上追加，Edge 网页图片（虚拟文件）同样处理；混入非图片时只提示忽略数量，不报错。**源文件必须保留**（只复制，不移动）；浏览器落在 `%TEMP%\hexbench-drop\` 的临时文件也应能复制进来。
- 发送：先点击记事本或浏览器输入框，再按 Alt+1 呼出面板 → 便签 → 发送。面板应收起，焦点回到原窗口，文字逐字输入；正文中的 `![...](...)` 不应被输入。多行、中文、emoji 都要验证。
  **多行内容要在记事本中核对行数**：换行必须实际发送回车（`type_text` 将 `\n` 作为按键发送），不能作为 `U+000A` 字符写入，否则控件会丢弃该字符，整段内容挤成一行。
  管理员权限的窗口会失败：面板应自行恢复并提示，不能静默丢字。
- 修改便签目录或开关后无需重启：设置页的「保存位置」立即显示新目录，面板侧边栏的便签条数同步变化。保存带有 300 ms 防抖，界面不要用后端回读的路径覆盖刚选的值。
- 「复制图片」后在资源管理器中按 Ctrl+V，应得到该笔记的全部图片文件。
- 删除和清空通过回收站完成（`trash_quiet`），回收站中可以找到。
- 回归：`dist/` 里不应再有 `phrases.html`；托盘没有「常用语」；`Ctrl+Alt+Shift+Q` 无反应；
  旧配置里的 `phrases` 段读入后不报错、下次保存自动消失。

### 钉在桌面上的便签

- 点卡片上的钉子 → 桌面出现一个 `note-N` 小窗（始终置顶、任务栏里没有），
  `config.json` 的 `notes.pinned` 里多一条 `{ id, label, x, y, w, h }`。再点一次钉子（或窗内
  「取消钉住」）窗关掉、这条配置消失。同一个位置首次打开不能盖住呼出面板。
- 窗里默认是排版视图：标题、粗体 / 斜体 / 行内代码、列表、引用、链接，图片走
  `hbnote://` 显示原图。双击正文或点铅笔进入编辑，**没有保存按钮**；停下约 0.5 秒后
  `.md` 文件应已更新，标题栏显示「已保存」。Ctrl+Z / Ctrl+Y 撤销重做，撤销后的内容也要存盘。
- 改标题会连带改文件名，`notes.pinned` 里的 `id` 要跟着换，窗口不能重建。
- 收起：双击标题栏、或点右上角的下箭头，窗口缩成一条 38px 的标题栏（只剩标题、展开箭头、
  取消钉住），`rolled: true`；再双击 / 点箭头展开，回到原来的高度。**收起 / 展开后窗口不能
  消失**（`wins.ps1` 里 `vis` 要还是 True）——便签窗是用 `ShowWindow(SW_SHOWNOACTIVATE)`
  显示的，tao 内部记得的还是「隐藏」，改窗口样式会把 `WS_VISIBLE` 抹掉，`note_roll` 里
  改完样式要再显示一次。收起时拖动只改 `x / y`，`w / h` 保持展开时的值；
  重启后仍是收起状态和同样的宽度。
- 拖动窗子、拉边改大小，松手后 `notes.pinned` 的 `x / y / w / h` 更新；
  退出再启动，窗子回到同一位置同一大小（尺寸和位置都夹在工作区内）。
- 删掉被钉住的笔记、清空收藏、在面板上取消钉住 → 对应窗口立刻关掉，配置里不留死条目；
  Alt+F4 关窗等于取消钉住。**从托盘退出程序再启动，钉住的便签必须还在**（退出不能当成取消钉住）。
- 深色主题下窗内文字、边框、输入框都跟着变（和面板一致）。

## 删除 / 回收站后的键盘焦点

删进回收站、撤销这类操作会把键盘焦点从 WebView2 收回到顶层的「Tauri Window」。
之后 `GetForegroundWindow` 仍然是面板，但页面收不到任何按键——表现就是删完立刻
按 Ctrl+Z 没反应，而过了几秒点别处再点回来又好了。

- `launcher_refocus` 在删除 / 撤销后延迟 180 ms 抢前台，并调
  `sys::focus_webview`：找到 WebView2 的 `Chrome_WidgetWin_1` 子窗口，
  `AttachThreadInput` 之后 `SetFocus`（光 `SetFocus` 顶层窗口没用）。上次成功
  聚焦的子窗口会记在 `sys.rs` 的静态量里，面板里开内置网页时也不会把焦点交给网页那一半。
- 复现：面板里点一个文件 → Delete → 0.5 s 内按 Ctrl+Z。
  判定：文件从回收站回来了（日志 `undo: 已从回收站还原 1 个文件`），全程不点面板。
- 排查技巧：`GetGUIThreadInfo(fgThread)` 的 `hwndFocus` 应该是一个
  `Chrome_WidgetWin_1`，如果等于顶层窗口句柄（`class=Tauri Window`）就是焦点丢了；
  此时直接向页面打字也不会出现字符。

## 呼出 / 收起的压测

此前的卡死问题都出现在这里。修改 `reveal_launcher` / `hide_launcher` 后都需要执行一遍：

```
连按 Alt+1 40～60 次，间隔 40～80ms
```

- 使用 `keybd_event` 发送，不要使用模拟鼠标点击（面板是 always_on_top，点击容易落到其他窗口）。
- 判定：每次按键后 `IsWindowVisible` 应严格在 1 / 0 之间交替；日志中不应出现
  `ui: main thread busy`（看门狗在 ≥400ms 时记录一行，括号内为当时阻塞的操作）。
- 最后再按一次应能正常呼出，并且 `GetForegroundWindow` 为面板窗口（表示可以输入）。

**注意事项**：窗口操作（`show` / `hide` / `set_focus` / `SetForegroundWindow`）
只能在窗口所属的线程上执行。从其他线程调用 `SetFocus` 会跨线程同步发送
`WM_KILLFOCUS` / `WM_SETFOCUS`；主线程正在处理键盘消息（tao 在其中持有
`KEY_EVENT_BUILDERS`）时双方互等，会导致面板僵死、快捷键失效。
需要延迟执行时，在工作线程中 `sleep`，再通过 `run_on_main_thread` 回到主线程。

## 右键菜单里的系统命令（压缩 / 复制 / 发送到）

面板菜单选择系统命令后，`InvokeCommand` 只是将任务交给 Shell，命令自身还会弹出进度窗口并抢占前台。此期间面板必须保留：

- `show_shell_context_menu` 在 `InvokeCommand` **之前**设置 `SHELL_OP_UNTIL`
  （`_ms` 时间戳），`watch_shell_op` 监视目标目录的条目数、最新修改时间和总大小，
  仍在变化则续期；连续 10 次轮询（2.5 秒）无变化即视为完成。
- 此期间收到的失焦不收起面板，改由 `hide_after_shell_op` 在命令结束后补一次收起
  （前提是当时前台确实不在面板上）。

验证：在 `D:\hexbench-test\` 中临时放置数百 MB 的目录，右键 →「添加到 xxx.zip」，
日志中 `invoked shell command` 之后**不应**紧跟 `hide`，压缩结束后才收起。
自动化可以使用 `%TEMP%\menutest.ps1`（弹出菜单、按 Down×N + Enter 选中）。

## 图标（lucide 一套）

界面图标（不含文件类型图标）只有一个来源：**lucide**。

- 网页部分直接用 `lucide-react`：设置窗口、呼出面板都是。
- 桌面格子由 Direct2D 绘制，无法直接使用 SVG：`node scripts/gen-icons.mjs` 读取
  `node_modules/lucide-react` 中的图标定义，将 path / circle / rect 图元采样为折线，
  写入 `crates/hexbench-fence/src/icons.rs`（**生成文件，不要手动修改**）。更换图标或升级
  lucide 后需重新运行；CI 会重新生成并比对，文件过期将直接失败。
- 原生层按 `size / 24` 缩放，描边宽度相应为 `2 / 24 * size`，与网页中
  `<Icon size={n} />` 的比例一致；使用圆头端点、圆角连接（`gpu.stroke`）。
- 尺寸只有三档：工具条 / 标题条按钮 **16**（`size={16}`、`*_ICON = 16.0`），
  前进 / 后退箭头和卡片头图标 **18**，文字按钮旁的小图标 **14**。三处要一起改。
- 新增图标：在 `scripts/gen-icons.mjs` 的 `ICONS` 里加一行（Rust 常量名 → lucide
  文件名），重跑脚本，再在 `win.rs` 里 `bake_icon` 并按需 `draw_icon`。
- 文件 / 文件夹图标是系统图标（`hbicon://`），不属于这套，不要试图替换。

## 调试版 exe 的前端来源

`cargo build` 生成的 debug exe **不内嵌前端**：主界面、面板等 WebView 窗口都从
`http://127.0.0.1:1420`（`devUrl`）加载，因此必须先运行 `pnpm tauri dev`（或 `pnpm dev` 启动 Vite）。未启动 Vite 时，每个窗口都会显示 WebView2 的「无法访问此页面」，界面中没有其他提示。
桌面格子不是 WebView（见 `crates/hexbench-fence`），不受此限制。

手动验证或交给其他人试用时，使用内嵌前端的版本：

```
pnpm build
pnpm tauri build --debug --no-bundle --config '{"build":{"beforeBuildCommand":""}}'
```

产物还是 `target/debug/hexbench-app.exe`（桌面右键菜单不用重装），窗口 URL 变成
`http://tauri.localhost/fence.html?id=…`。这条路吃的是打包后的 `dist`（React 生产
构建），之后改前端要重新跑一遍这两条命令。

## 验证常用手法

| 目的 | 手法 |
| --- | --- |
| 快捷键是否触发 | 看日志里的 `clipboard:` / `trash:` / `bg menu:` 行 |
| 文件操作是否成功 | 直接 `Get-ChildItem` 看磁盘 |
| 窗口尺寸/位置 | 枚举窗口 + `GetWindowRect` |
| 刷新是否及时 | 操作后截图看列表，或读日志 |
| 主线程卡在哪 | 看 `ui: main thread busy … (stuck in …)`，对应 `lib.rs` 里的 `op_name` |
