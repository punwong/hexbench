import { invoke } from "@tauri-apps/api/core";

/// 主题偏好只有三种：system（跟系统）/ light / dark。后端只存偏好，
/// system 由每个页面自己看 prefers-color-scheme —— 窗口不重新加载时，
/// 系统的深浅切换也只有页面自己知道。
export function applyTheme(preference: string) {
  const dark =
    preference === "dark" ||
    (preference === "system" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

/// 页面起来时读一次后端偏好，之后系统主题变了再算一遍。
/// 在设置里直接改偏好不用页面操心：后端会往每个常驻窗口推一次 data-theme
/// （见 push_theme_to_windows），否则格子 / 面板要等下次开窗才变色。
export function watchTheme(): () => void {
  const apply = () => {
    void invoke<string>("ui_theme_preference")
      .then((preference) => applyTheme(preference))
      .catch(() => undefined);
  };
  apply();
  const query = window.matchMedia("(prefers-color-scheme: dark)");
  query.addEventListener("change", apply);
  return () => query.removeEventListener("change", apply);
}
