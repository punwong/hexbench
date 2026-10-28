// 桌面应用不该带着浏览器的那套行为。
//
// WebView2 本质上还是浏览器：默认允许选中文字、右键弹出「刷新 / 另存为 / 检查」、
// Ctrl+滚轮缩放页面、F5 刷新丢状态。这些在网页里正常，在桌面应用里都是缺陷。
// 这里统一关掉，输入框内保持原样。

const SELECTABLE = "input, textarea, [contenteditable='true']";

function isSelectable(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(SELECTABLE) !== null;
}

// 右键菜单：输入框里保留系统菜单（复制/粘贴），其他位置不弹。
window.addEventListener("contextmenu", (event) => {
  if (isSelectable(event.target)) return;
  event.preventDefault();
});

// Ctrl+滚轮：别缩放整个界面。
window.addEventListener(
  "wheel",
  (event) => {
    if (event.ctrlKey) event.preventDefault();
  },
  { passive: false },
);

// F5 / Ctrl+R 会重新加载页面，把当前状态全部丢掉；缩放快捷键同理。
window.addEventListener("keydown", (event) => {
  const key = event.key.toLowerCase();
  const withCtrl = event.ctrlKey || event.metaKey;
  if (event.key === "F5" || (withCtrl && ["r", "=", "-", "0", "+"].includes(key))) {
    event.preventDefault();
  }
});
