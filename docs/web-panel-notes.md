# 网页面板（右侧分栏）实现笔记

> 状态：**已完成**（0.4.0）。网页是独立的窗口，不是面板内的分栏。
> 记录到这里，避免下次从零开始。

## 目标形态

```
一个窗口（宽度可变）
┌────────┬──────────────┬──────────────┐
│ 侧边栏 │  文件区(不变) │  网页区 420  │  ← 打开时窗口宽度 +420
│ 分类   │              │  图标标签页  │
│ 常用夹 │              │              │
└────────┴──────────────┴──────────────┘
```

网页内容不是 iframe（会被 `X-Frame-Options` 拒绝），而是**挂在同一个窗口里的子 WebView**。

## 关键实现点

| 位置 | 作用 |
| --- | --- |
| `sync_web_side(app, open)` | 唯一的开关：算宽度（基准 +420）、`set_size` 窗口、创建/定位/显示子 WebView |
| `reflow_web_page(app)` | 窗口 `Resized` 时把子 WebView 重新铺到右栏（**不能靠 auto_resize**） |
| `WEB_COLUMN_WIDTH = 420.0` | 右栏宽度 |
| `WEB_TOOLBAR = 40.0` | 顶栏高度，必须和 `launcher.html` 的 `.toolbar` 一致 |
| `place_launcher` | 呼出时若 `WEB_PANEL_OPEN`，宽度额外 +420（否则每次呼出都会缩回去） |
| `launcher.html` `.web-col` | `display:none`，`body.web-open` 时 `display:flex` |
| `launcher.tsx` | 用 `document.body.classList` 控制右栏，不走 React 状态 |

## 三条必须知道的 Tauri issue

1. **[#13318](https://github.com/tauri-apps/tauri/issues/13318)** 父窗口缩放时子 WebView **不会自动跟随**；官方给的临时解法是监听 resize 自己 `set_size`。
2. **[#9611](https://github.com/tauri-apps/tauri/issues/9611)**（仍 open）`auto_resize()` 与**固定位置**不能同时用——用了它横向偏移会失效，右栏定位就废了。
3. **[#10053](https://github.com/tauri-apps/tauri/issues/10053)** 手动 `set_size` 在坐标非 `(0,0)` 时曾摆放错误（已修）。

官方参考示例：[tauri/examples/multiwebview/main.rs](https://github.com/tauri-apps/tauri/blob/dev/examples/multiwebview/main.rs)（一个窗口 + 多个子 WebView 分栏）。

## 踩过的坑

- **在主线程同步命令里创建窗口** → 白屏（WebView2 deadlock，见 wry#583）。必须 `async` 命令或独立线程。
- **每次「关掉再建」窗口** → Tauri 注册表与真实 HWND 脱节，窗口看得见但 `hide()` 无效；`build()` 的返回值才是可靠句柄。
- **两个独立窗口拼接** → 边框粗细/颜色永远对不齐（8px 隐形外框 + 两条边框叠加），并且收起时容易遗留一个窗口。
- **`transparent(true)` + 无边框窗口** 下，窗口尺寸变化不一定会让内部页面重新布局，需要额外处理。

## 下一步（按顺序）

1. 让 `web_panel_show/hide` 成为**唯一**的宽度来源，前端只反映结果（不再各自算宽度）。
2. 打开时顺序改成：先加宽窗口 → 等一次 resize → 再创建子 WebView（避免子 WebView 按旧宽度摆放）。
3. 子 WebView 创建后立即日志输出 `add_child` 的结果，确认是成功还是被拒。
4. 用真实点击验证「打开 → 收起 → 再呼出」三态尺寸稳定（960 / 1380 / 1380）。

## 备选方案

如果子 WebView 路线继续卡住，退回**独立网页窗口**：那个版本实测能正常渲染网页与图标标签，遗留问题只有边框粗细不一致，可用“两窗都无边框 + 面板右边框当分隔线”绕过。
