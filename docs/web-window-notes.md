# 网页窗口实现笔记

> 状态：**已完成**，随 0.5.0 发布。
> 这里记录**当前**实现和踩过的坑，避免下次从零开始。

## 形态

网页是**独立窗口**，贴在屏幕一侧或面板旁边，**不是**面板里的分栏。

```
┌──────────────┐  ┌────────────────┐
│  呼出面板     │  │  网页窗口       │
│  分类/常用夹  │  │  顶栏：图标标签页 │
│  文件区       │  │  ─────────────  │
│              │  │  子 WebView     │
└──────────────┘  └────────────────┘
   50%×50%          24%×85%（可拖）
```

窗口由 `web.html`（顶栏 + 标签页，纯内联脚本，没有 React）承载，站点内容是
挂在这个窗口里的**子 WebView**——不是 iframe（会被 `X-Frame-Options` 拒绝）。

## 关键函数（`src-tauri/src/lib.rs`）

| 函数 | 作用 |
| --- | --- |
| `ensure_web_window` | 建窗口（只在 `setup` 里调一次，建完立刻隐藏） |
| `place_web_window` | 按配置摆放：跟随面板 / 记住的几何 / 固定一侧 |
| `reflow_site_page` | 把子 WebView 按窗口尺寸重新铺满（顶栏以下） |
| `remember_web_geometry` | 窗口移动/缩放后记几何（800ms 防抖） |
| `store_web_geometry` | 真正写配置（摆放后强制调一次） |
| `sync_web_side` | 面板收起/呼出时跟着显示或隐藏 |

## 摆放优先级

```
跟随面板（siteFollowPanel）
  └─ 是 → 贴面板右侧 + 8px，尺寸按百分比
  └─ 否 → 记住了位置和大小（siteRememberPosition 且百分比没改过）→ 用记住的
          否则 → 按百分比 + 左/右侧
```

改设置里的百分比会让记住的几何**作废**（存了 `sitePctW/H` 做快照比对）。

## 相关配置

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `launcher.webEnabled` | `true` | 内置网页总开关 |
| `launcher.sites` | `[]` | 站点列表（名字 + 网址） |
| `launcher.siteKeepAliveSecs` | `90` | 面板收起后网页保留秒数 |
| `launcher.siteWidthPercent` / `siteHeightPercent` | `24` / `85` | 占工作区百分比 |
| `launcher.siteFollowPanel` | `false` | 跟随面板 |
| `launcher.siteSide` | `right` | 固定时贴哪侧 |
| `launcher.siteRememberPosition` | `true` | 记住拖动后的位置和大小 |
| `launcher.siteX/Y/W/H` | `0` | 记住的几何（逻辑像素） |
| `launcher.sitePctW/H` | `0` | 保存几何时的百分比快照 |

## 踩过的坑（按疼痛程度排序）

1. **窗口建好了但查不到**
   `get_webview_window("web")` 永远返回 `None`，但 webview 本身注册成功了、
   窗口也看得见。`build()` 的返回值是唯一可靠句柄，必须自己存下来
   （`static WEB_WINDOW: Mutex<Option<WebviewWindow>>`）。

2. **在同步命令里建窗口 → 白屏**
   WebView2 会死锁（[wry#583](https://github.com/tauri-apps/wry/issues/583)）。
   只能在 `setup` 或独立线程里建。

3. **子 WebView 不跟随父窗口**
   [tauri#13318](https://github.com/tauri-apps/tauri/issues/13318)。
   `auto_resize()` 不能用——它和固定位置冲突
   （[tauri#9611](https://github.com/tauri-apps/tauri/issues/9611)），
   必须自己监听 `Resized` 然后 `set_size`。
   症状就是「网页半边空白 / 内容整体右移」。

4. **透明 + 无边框窗口改尺寸不重排**
   [tauri#13906](https://github.com/tauri-apps/tauri/issues/13906)。
   面板因此改成 `transparent(false)`。

5. **无边框窗口拖不动、也拖不大**
   - 移动：顶栏要标 `data-tauri-drag-region`，否则拖不动
   - 缩放：系统不会自动给无边框窗口加缩放边框，要么自己监听边缘，
     要么放几个热区调 `startResizeDragging(direction)`
   - 面板和网页窗口现在都用「8 个透明热区」的方案

6. **隐藏/显示时被存下临时坐标**
   `hide()`/`show()` 过程中系统会报一次临时位置（例如 `130,130`），
   被当成用户拖动写进配置，下次呼出就跑到角落。
   → 加 `is_visible()` 判断，收起时再按窗口实际几何存一次。

7. **面板缩放会牵连网页**
   面板改大小时，网页窗口里的子 WebView 不会自己重排。
   → 面板 `Resized` 后防抖 220ms 再 `reflow_site_page`。

8. **`add_child` 在 `Window` 上，不在 `WebviewWindow` 上**
   要用 `web_panel.as_ref().window()` 换出底层 `Window`。

## 官方参考

[tauri/examples/multiwebview/main.rs](https://github.com/tauri-apps/tauri/blob/dev/examples/multiwebview/main.rs)
（一个窗口 + 多个子 WebView 分栏）

## 历史方案（已放弃）

最开始想在**面板内部**加一栏网页（打开时窗口宽度 +420）。放弃的原因：

- 面板宽度会被历史逻辑改来改去，尺寸不稳定
- 两个独立窗口拼接时边框永远对不齐（8px 隐形外框 + 两条边框叠加）
- 面板收起时容易遗留一个孤儿窗口

独立窗口方案没有这些问题，而且网页窗口可以比面板高得多。
