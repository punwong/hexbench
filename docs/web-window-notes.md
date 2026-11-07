# 内置网页窗口实现说明

本文描述快捷面板内置网页窗口的结构、摆放规则、配置项和实现约束。面向需要维护或修改该功能的开发者。

## 结构

网页是**独立窗口**，贴在屏幕一侧或面板旁边，不是面板内部的分栏。

```
┌──────────────┐  ┌────────────────┐
│  快捷面板     │  │  网页窗口       │
│  分类/常用夹  │  │  顶栏：站点标签页 │
│  文件区       │  │  ─────────────  │
│              │  │  子 WebView     │
└──────────────┘  └────────────────┘
   50%×50%          24%×85%（可拖动缩放）
```

- 窗口本身由 `web.html` 承载（顶栏、标签页，纯内联脚本，不使用 React）。
- 站点内容以**子 WebView** 的形式挂在该窗口内。不使用 iframe，因为多数站点通过 `X-Frame-Options` 或 CSP 禁止被嵌入。

## 代码结构

实现位于 `src-tauri/src/lib.rs`。

| 函数 | 作用 |
| --- | --- |
| `ensure_web_window` | 创建窗口，只在 `setup` 中调用一次，创建后立即隐藏 |
| `place_web_window` | 按配置摆放：跟随面板、使用记住的几何，或固定在一侧 |
| `reflow_site_page` | 按窗口尺寸重新铺满子 WebView（顶栏以下区域） |
| `remember_web_geometry` | 窗口移动或缩放后记录几何（800ms 防抖） |
| `store_web_geometry` | 写入配置；每次摆放后强制调用一次 |
| `sync_web_side` | 面板收起或呼出时同步显示或隐藏网页窗口 |

## 摆放规则

```
跟随面板（siteFollowPanel）
  └─ 是 → 贴面板右侧 + 8px，尺寸按百分比
  └─ 否 → 存在记住的几何（siteRememberPosition 且百分比未改过）→ 使用记住的位置和大小
          否则 → 按百分比和左/右侧摆放
```

修改设置中的宽高百分比会使记住的几何失效。保存几何时同时写入百分比快照（`sitePctW/H`），用于判断百分比是否变化。

## 配置项

| 键 | 默认值 | 说明 |
| --- | --- | --- |
| `launcher.webEnabled` | `true` | 内置网页总开关 |
| `launcher.sites` | `[]` | 站点列表（名称 + 网址） |
| `launcher.siteKeepAliveSecs` | `90` | 面板收起后网页保留的秒数 |
| `launcher.siteWidthPercent` / `siteHeightPercent` | `24` / `85` | 占工作区的百分比 |
| `launcher.siteFollowPanel` | `false` | 是否跟随面板 |
| `launcher.siteSide` | `right` | 固定摆放时贴哪一侧 |
| `launcher.siteRememberPosition` | `true` | 是否记住拖动后的位置和大小 |
| `launcher.siteX/Y/W/H` | `0` | 记住的几何（逻辑像素） |
| `launcher.sitePctW/H` | `0` | 保存几何时的百分比快照 |

## 实现约束

以下限制来自 WebView2 与 Tauri 当前的实现方式，修改相关代码时需要遵守。

1. **窗口句柄需要自行保存。**
   `get_webview_window("web")` 可能返回 `None`，即使 webview 注册成功且窗口可见。`build()` 的返回值是唯一可靠的句柄，需保存到 `static WEB_WINDOW: Mutex<Option<WebviewWindow>>`。

2. **创建窗口必须在 `setup` 或独立线程中进行。**
   在同步命令里创建会导致 WebView2 死锁（[wry#583](https://github.com/tauri-apps/wry/issues/583)），表现为白屏。

3. **子 WebView 不会跟随父窗口 resize。**
   `auto_resize()` 与固定位置冲突（[tauri#13318](https://github.com/tauri-apps/tauri/issues/13318)、[tauri#9611](https://github.com/tauri-apps/tauri/issues/9611)）。需要监听 `Resized` 事件后自行调用 `set_size`，否则会出现网页半屏空白或内容整体偏移。

4. **透明无边框窗口修改尺寸后不会重新排布。**
   见 [tauri#13906](https://github.com/tauri-apps/tauri/issues/13906)。因此面板窗口使用 `transparent(false)`。

5. **无边框窗口需要自行实现拖动与缩放。**
   顶栏必须标记 `data-tauri-drag-region` 才能拖动；系统不会为无边框窗口添加缩放边框，需要监听边缘，或放置热区并调用 `startResizeDragging(direction)`。面板和网页窗口目前都使用 8 个透明热区。

6. **隐藏和显示过程中会产生临时坐标。**
   `hide()` / `show()` 期间系统可能上报一次临时位置（例如 `130,130`）。若直接写入配置，下次呼出时窗口会出现在错误位置。写入前需用 `is_visible()` 判断，并在收起时按窗口实际几何再保存一次。

7. **面板缩放后需要重新排布子 WebView。**
   面板尺寸变化不会自动影响网页窗口内的子 WebView。面板 `Resized` 后防抖 220ms 再调用 `reflow_site_page`。

8. **子 WebView 添加到 `Window` 而非 `WebviewWindow`。**
   需要通过 `web_panel.as_ref().window()` 取得底层 `Window` 后再调用 `add_child`。

## 参考

- [tauri/examples/multiwebview/main.rs](https://github.com/tauri-apps/tauri/blob/dev/examples/multiwebview/main.rs)：单窗口内多个子 WebView 分栏的官方示例。
